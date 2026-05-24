//! Orchestrate a query end-to-end: scan vault → filter → sort → limit → select.
//!
//! Everything is composed at the LazyFrame level; the only materialisation is
//! the final `.collect()` returning a `DataFrame` to the output layer.

use std::path::Path;

use polars::prelude::*;

use crate::base_file::{BaseFile, SortDirection, View};
use crate::error::Result;
use crate::expr::{TranslateCtx, parse, translate};
use crate::filter::combine_filters;
use crate::vault::{VaultSchema, scan_vault_to_lazyframe};

/// Execute a query against the vault for a given view, returning the result as
/// a polars `DataFrame`. Column order matches `view.order`.
pub fn execute_query(vault_root: &Path, base_file: &BaseFile, view: &View) -> Result<DataFrame> {
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
    let df = lf.select(column_exprs).collect()?;
    Ok(df)
}

/// Build the list of polars expressions to sort by. Always appends a
/// tie-breaker on `file_name` ascending.
fn sort_exprs(
    view: &View,
    ctx: &TranslateCtx,
    schema: &VaultSchema,
) -> Result<Option<(Vec<Expr>, Vec<bool>)>> {
    let mut exprs: Vec<Expr> = Vec::new();
    let mut descending: Vec<bool> = Vec::new();

    if let Some(gb) = &view.group_by {
        let e = property_to_expr(&gb.property, ctx, schema)?;
        exprs.push(e);
        descending.push(matches!(gb.direction, SortDirection::Desc));
    }
    if let Some(sorts) = &view.sort {
        for key in sorts {
            let e = property_to_expr(&key.property, ctx, schema)?;
            exprs.push(e);
            descending.push(matches!(key.direction, SortDirection::Desc));
        }
    }

    if exprs.is_empty() {
        return Ok(None);
    }
    // Tie-break by file_name ascending (matches old behavior).
    exprs.push(col("file_name"));
    descending.push(false);
    Ok(Some((exprs, descending)))
}

/// Convert a property name (as appears in `groupBy.property` or `sort[].property`)
/// to a polars expression. Supports `file.X`, `note.X`, `formula.X`, and bare
/// frontmatter keys.
fn property_to_expr(prop: &str, ctx: &TranslateCtx, schema: &VaultSchema) -> Result<Expr> {
    if let Some(rest) = prop.strip_prefix("file.") {
        let col_name = format!("file_{rest}");
        if schema.has_column(&col_name) {
            return Ok(col(col_name));
        }
        return Ok(lit(NULL));
    }
    if let Some(rest) = prop.strip_prefix("note.") {
        if let Some(name) = schema.resolve_frontmatter(rest) {
            return Ok(col(name.to_string()));
        }
        return Ok(lit(NULL));
    }
    if let Some(rest) = prop.strip_prefix("formula.") {
        if let Some(body) = ctx.formulas.get(rest) {
            let ast = parse(body)?;
            let t = translate(&ast, ctx)?;
            return Ok(t.expr);
        }
        return Ok(lit(NULL));
    }
    if let Some(name) = schema.resolve_frontmatter(prop) {
        return Ok(col(name.to_string()));
    }
    Ok(lit(NULL))
}

/// Build the `select` expressions, one per `view.order` column, applying the
/// `title` special case and the rename to the output header.
fn column_select_exprs(view: &View, ctx: &TranslateCtx, schema: &VaultSchema) -> Result<Vec<Expr>> {
    let order = view.order.clone().unwrap_or_default();
    order
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
            return Ok(t.expr);
        }
        return Ok(lit(NULL));
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
