//! Orchestrate a query end-to-end: scan vault → filter → sort → limit → select.
//!
//! Everything is composed at the LazyFrame level; the only materialisation is
//! the final `.collect()` returning a `DataFrame` to the output layer.

use std::path::Path;

use polars::prelude::*;

use crate::base_file::{BaseFile, SortDirection, View};
use crate::error::Result;
use crate::expr::{InferredType, TranslateCtx, parse, translate};
use crate::filter::combine_filters;
use crate::vault::{VaultSchema, obsidian_sort_key, scan_vault_to_lazyframe};

/// Build the lazy query for a view without materialising it. Returns a
/// `LazyFrame` whose schema matches `view.order`; the caller chooses when (or
/// whether) to `.collect()`. This is the form the Python bindings expose.
pub fn build_query_lazy(vault_root: &Path, base_file: &BaseFile, view: &View) -> Result<LazyFrame> {
    let (mut lf, schema) = scan_vault_to_lazyframe(vault_root)?;
    let ctx = TranslateCtx::new(&schema, &base_file.formulas);

    let predicate = combine_filters(base_file.filters.as_ref(), view.filters.as_ref(), &ctx)?;
    lf = lf.filter(predicate);

    if let Some((exprs, descending)) = sort_exprs(view, &ctx, &schema)? {
        lf = lf.sort_by_exprs(
            exprs,
            SortMultipleOptions::new()
                .with_order_descending_multi(descending)
                .with_nulls_last(true)
                .with_maintain_order(true),
        );
    }

    if let Some(n) = view.limit {
        lf = lf.limit(n as u32);
    }

    let column_exprs = column_select_exprs(view, &ctx, &schema)?;
    Ok(lf.select(column_exprs))
}

/// Execute a query against the vault for a given view, returning the result as
/// a polars `DataFrame`. Column order matches `view.order`.
pub fn execute_query(vault_root: &Path, base_file: &BaseFile, view: &View) -> Result<DataFrame> {
    let df = build_query_lazy(vault_root, base_file, view)?.collect()?;
    Ok(df)
}

/// Build the list of polars expressions to sort by. Always appends a
/// tie-breaker on `file_name` ascending. If no explicit sort is given, fall
/// back to sorting by the first column in `order` (matching Obsidian's
/// implicit default).
fn sort_exprs(
    view: &View,
    ctx: &TranslateCtx,
    schema: &VaultSchema,
) -> Result<Option<(Vec<Expr>, Vec<bool>)>> {
    let mut exprs: Vec<Expr> = Vec::new();
    let mut descending: Vec<bool> = Vec::new();

    if let Some(gb) = &view.group_by {
        let e = sort_expr_for(&gb.property, ctx, schema)?;
        exprs.push(e);
        descending.push(matches!(gb.direction, SortDirection::Desc));
    }
    if let Some(sorts) = &view.sort {
        for key in sorts {
            let e = sort_expr_for(&key.property, ctx, schema)?;
            exprs.push(e);
            descending.push(matches!(key.direction, SortDirection::Desc));
        }
    }

    if exprs.is_empty() {
        if let Some(first) = view.order.as_ref().and_then(|o| o.first()) {
            let e = sort_expr_for(first, ctx, schema)?;
            exprs.push(e);
            descending.push(false);
        } else {
            return Ok(None);
        }
    }
    // Tie-break by file_path then file_name ascending. We sort on the
    // precomputed `*_natkey` columns so the order matches Obsidian's
    // `localeCompare(_, {numeric: true})`: numeric runs sort numerically,
    // and punctuation keeps its code-point weight (so e.g. "Study Notes.md"
    // sorts before "Study.md" via the space vs period comparison).
    exprs.push(col("file_path_natkey"));
    descending.push(false);
    exprs.push(col("file_name_natkey"));
    descending.push(false);
    Ok(Some((exprs, descending)))
}

/// Build a stable, Obsidian-compatible sort key from a string expression.
/// Per-row, replaces digit runs with 16-char zero-padded versions and
/// lowercases everything else — same shape as the file_path_natkey column,
/// matching Obsidian's `localeCompare(_, {numeric: true})`. Implemented as
/// an Expr::map so it can wrap arbitrary string-valued exprs (including
/// the output of formulas).
fn string_sort_key(e: Expr) -> Expr {
    e.cast(DataType::String).map(
        |c: Column| {
            let s = c.as_materialized_series();
            let ca = s.str()?;
            let out: StringChunked = ca
                .into_iter()
                .map(|opt| opt.map(obsidian_sort_key))
                .collect();
            let out = out.with_name(s.name().clone());
            Ok(Some(out.into_column()))
        },
        GetOutput::from_type(DataType::String),
    )
}

/// Translate a sort property into a (sort-key expr, inferred type) pair.
/// For string-typed values we apply `string_sort_key` so the order matches
/// Obsidian; for any other type we return the value as-is.
fn sort_expr_for(prop: &str, ctx: &TranslateCtx, schema: &VaultSchema) -> Result<Expr> {
    let (base, is_string) = sort_base_expr(prop, ctx, schema)?;
    if is_string {
        Ok(string_sort_key(base))
    } else {
        Ok(base)
    }
}

fn sort_base_expr(prop: &str, ctx: &TranslateCtx, schema: &VaultSchema) -> Result<(Expr, bool)> {
    if let Some(rest) = prop.strip_prefix("file.") {
        let col_name = format!("file_{rest}");
        if schema.has_column(&col_name) {
            let is_string = matches!(schema.dtype(&col_name), Some(DataType::String));
            return Ok((col(col_name), is_string));
        }
        return Ok((null_column_expr(), false));
    }
    if let Some(rest) = prop.strip_prefix("note.") {
        if let Some(name) = schema.resolve_frontmatter(rest) {
            let is_string = matches!(schema.dtype(name), Some(DataType::String));
            return Ok((col(name.to_string()), is_string));
        }
        return Ok((null_column_expr(), false));
    }
    if let Some(rest) = prop.strip_prefix("formula.") {
        if let Some(body) = ctx.formulas.get(rest) {
            let ast = parse(body)?;
            let t = translate(&ast, ctx)?;
            let is_string = matches!(t.ty, InferredType::String);
            return Ok((broadcast_to_frame(t.expr), is_string));
        }
        return Ok((null_column_expr(), false));
    }
    if let Some(name) = schema.resolve_frontmatter(prop) {
        let is_string = matches!(schema.dtype(name), Some(DataType::String));
        return Ok((col(name.to_string()), is_string));
    }
    Ok((null_column_expr(), false))
}

/// A null-valued expression with the LazyFrame's row count. Built by taking a
/// known-present column (`file_path`) and substituting null for every row.
fn null_column_expr() -> Expr {
    when(col("file_path").is_not_null())
        .then(lit(NULL))
        .otherwise(lit(NULL))
}

/// If `e` happens to evaluate to a scalar (e.g. an unsupported AST node that
/// collapsed to `lit(NULL)`), wrap it so polars broadcasts it across the
/// LazyFrame. Safe to apply to columnar exprs — they pass through unchanged.
fn broadcast_to_frame(e: Expr) -> Expr {
    when(col("file_path").is_not_null())
        .then(e.clone())
        .otherwise(e)
}

/// Build the `select` expressions, one per `view.order` column, applying the
/// `title` special case and the rename to the output header.
fn column_select_exprs(view: &View, ctx: &TranslateCtx, schema: &VaultSchema) -> Result<Vec<Expr>> {
    view.order
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|c| column_to_expr(c, ctx, schema).map(|e| e.alias(c.as_str())))
        .collect()
}

fn column_to_expr(column: &str, ctx: &TranslateCtx, schema: &VaultSchema) -> Result<Expr> {
    if column == "title" {
        // Special: [[<file_path>| <title-or-stem>]]
        let display = if schema.has_column("title") {
            // Use frontmatter title if not null, else file_name (stem).
            when(col("title").is_not_null())
                .then(col("title").cast(DataType::String))
                .otherwise(col("file_name"))
        } else {
            col("file_name")
        };
        return Ok(concat_str(
            vec![lit("[["), col("file_path"), lit("| "), display, lit("]]")],
            "",
            true,
        ));
    }
    if let Some(rest) = column.strip_prefix("file.") {
        let col_name = format!("file_{rest}");
        if schema.has_column(&col_name) {
            return Ok(col(col_name));
        }
        return Ok(lit(NULL));
    }
    if let Some(rest) = column.strip_prefix("formula.") {
        if let Some(body) = ctx.formulas.get(rest) {
            let ast = parse(body)?;
            let t = translate(&ast, ctx)?;
            return Ok(broadcast_to_frame(t.expr));
        }
        return Ok(null_column_expr());
    }
    if let Some(rest) = column.strip_prefix("note.") {
        if let Some(name) = schema.resolve_frontmatter(rest) {
            return Ok(col(name.to_string()));
        }
        return Ok(lit(NULL));
    }
    if let Some(name) = schema.resolve_frontmatter(column) {
        return Ok(col(name.to_string()));
    }
    Ok(lit(NULL))
}
