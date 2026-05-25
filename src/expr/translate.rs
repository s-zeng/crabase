//! Compile the expression-language AST to a polars `Expr`.
//!
//! The translator walks each AST node *once* at query-build time, producing a
//! polars expression that polars evaluates lazily against the vault LazyFrame.
//! There is no per-row dispatch on a `Value` enum at runtime — every node
//! becomes a typed polars expression.
//!
//! Inferred types (`InferredType`) are tracked through the translation so that
//! method calls can pick the right polars namespace (e.g. `.length` on a string
//! → `str().len_chars()`, on a list → `list().len()`).

use std::collections::HashMap;

use chrono::{Local, NaiveDate, NaiveDateTime};
use polars::prelude::*;

use crate::error::{CrabaseError, Result};
use crate::expr::ast::{BinOp, Expr as AstExpr, ExprKind, Ident, Literal, UnaryOp};
use crate::vault::VaultSchema;

/// Best-effort static type tag attached to a translated expression. Used to
/// pick the right method namespace and to format outputs sensibly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferredType {
    Unknown,
    Null,
    Bool,
    Int,
    Float,
    String,
    Date,
    Datetime,
    Duration,
    List, // List of strings
}

impl InferredType {
    pub fn from_dtype(dt: &DataType) -> InferredType {
        match dt {
            DataType::Boolean => InferredType::Bool,
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64 => InferredType::Int,
            DataType::Float32 | DataType::Float64 => InferredType::Float,
            DataType::String => InferredType::String,
            DataType::Date => InferredType::Date,
            DataType::Datetime(_, _) => InferredType::Datetime,
            DataType::Duration(_) => InferredType::Duration,
            DataType::List(_) => InferredType::List,
            DataType::Null => InferredType::Null,
            _ => InferredType::Unknown,
        }
    }

    fn is_date_or_datetime(&self) -> bool {
        matches!(self, InferredType::Date | InferredType::Datetime)
    }

    fn is_numeric(&self) -> bool {
        matches!(self, InferredType::Int | InferredType::Float)
    }
}

/// Output of translating a single AST node.
#[derive(Debug, Clone)]
pub struct Translated {
    pub expr: Expr,
    pub ty: InferredType,
}

impl Translated {
    pub fn new(expr: Expr, ty: InferredType) -> Self {
        Self { expr, ty }
    }
}

/// Mutable state threaded through translation. Holds the vault schema (for
/// column dtype lookup), formulas (for inlining), a formula stack (for cycle
/// detection at compile time), and the `value` binding used inside
/// `list.eval` callbacks.
#[derive(Debug, Clone)]
pub struct TranslateCtx<'a> {
    pub schema: &'a VaultSchema,
    pub formulas: &'a HashMap<String, String>,
    formula_stack: Vec<String>,
    /// When set, references to the bare identifier `value` resolve to the
    /// implicit list-element column `col("")` inside a `list.eval` callback.
    value_bound: bool,
}

impl<'a> TranslateCtx<'a> {
    pub fn new(schema: &'a VaultSchema, formulas: &'a HashMap<String, String>) -> Self {
        Self {
            schema,
            formulas,
            formula_stack: Vec::new(),
            value_bound: false,
        }
    }

    fn with_formula(&self, name: &str) -> Result<Self> {
        if self.formula_stack.iter().any(|s| s == name) {
            let mut cycle = self.formula_stack.clone();
            cycle.push(name.to_string());
            return Err(CrabaseError::ExprEval(format!(
                "Formula cycle detected: {}",
                cycle.join(" -> ")
            )));
        }
        let mut next = self.clone();
        next.formula_stack.push(name.to_string());
        Ok(next)
    }

    fn with_value_binding(&self) -> Self {
        let mut next = self.clone();
        next.value_bound = true;
        next
    }
}

/// Top-level entry point. Translate one AST to a polars `Expr`.
pub fn translate(ast: &AstExpr, ctx: &TranslateCtx) -> Result<Translated> {
    translate_inner(ast, ctx)
}

fn translate_inner(ast: &AstExpr, ctx: &TranslateCtx) -> Result<Translated> {
    match &ast.kind {
        ExprKind::Literal(lit_node) => translate_literal(lit_node),
        ExprKind::Variable(name) => translate_variable(name, ctx),
        ExprKind::Member { object, field } => translate_member(object, field.as_str(), ctx),
        ExprKind::Index { object, index } => translate_index(object, index, ctx),
        ExprKind::Call { callee, args } => translate_call(callee, args, ctx),
        ExprKind::Array(items) => translate_array(items, ctx),
        ExprKind::Binary { op, left, right } => translate_binary(op, left, right, ctx),
        ExprKind::Unary { op, operand } => translate_unary(op, operand, ctx),
    }
}

// ---------- Literals ----------

fn translate_literal(lit_node: &Literal) -> Result<Translated> {
    Ok(match lit_node {
        Literal::Number(n) => {
            if n.fract() == 0.0 && n.abs() < (i64::MAX as f64) {
                Translated::new(lit(*n as i64), InferredType::Int)
            } else {
                Translated::new(lit(*n), InferredType::Float)
            }
        }
        Literal::Str(s) => Translated::new(lit(s.clone()), InferredType::String),
        // Regex literals collapse to their raw pattern string when seen on
        // their own. `.replace()` and friends sniff the original AST node to
        // pick regex-aware polars APIs.
        Literal::Regex(s) => Translated::new(lit(s.clone()), InferredType::String),
        Literal::Bool(b) => Translated::new(lit(*b), InferredType::Bool),
        Literal::Null => Translated::new(lit(NULL), InferredType::Null),
    })
}

// ---------- Variables ----------

fn translate_variable(name: &Ident, ctx: &TranslateCtx) -> Result<Translated> {
    let n = name.as_str();
    if n == "value" && ctx.value_bound {
        return Ok(Translated::new(col(""), InferredType::Unknown));
    }
    // Bare `file` (e.g. inside `link(file, ...)`) refers to the current file
    // and stringifies to its path. Obsidian's formula language treats it as a
    // first-class file handle, but for our purposes — building strings and
    // links — its path form is what callers want.
    if n == "file" {
        return Ok(Translated::new(col("file_path"), InferredType::String));
    }
    if ctx.formulas.contains_key(n) {
        return translate_formula(n, ctx);
    }
    if let Some(dt) = ctx.schema.dtype(n) {
        return Ok(Translated::new(col(n), InferredType::from_dtype(dt)));
    }
    Ok(Translated::new(lit(NULL), InferredType::Null))
}

fn translate_formula(name: &str, ctx: &TranslateCtx) -> Result<Translated> {
    let Some(body) = ctx.formulas.get(name) else {
        return Ok(Translated::new(lit(NULL), InferredType::Null));
    };
    let ast = crate::expr::parser::parse(body)?;
    let inner = ctx.with_formula(name)?;
    translate_inner(&ast, &inner)
}

// ---------- Member access (a.b) ----------

fn translate_member(object: &AstExpr, field: &str, ctx: &TranslateCtx) -> Result<Translated> {
    // Special-case top-level namespaces: file, note, formula
    if let ExprKind::Variable(obj_name) = &object.kind {
        match obj_name.as_str() {
            "file" => return translate_file_field(field, ctx),
            "note" => return translate_note_field(field, ctx),
            "formula" => return translate_formula(field, ctx),
            _ => {}
        }
    }

    // Cross-file property lookup: `<link>.asFile().properties.<X>`. We
    // resolve the link's stem against the vault's file_name column and pull
    // the X column's value for the matching row.
    if let Some(link_obj) = match_as_file_properties(object) {
        return translate_cross_file_property(link_obj, field, ctx);
    }

    let receiver = translate_inner(object, ctx)?;
    member_on_value(receiver, field)
}

/// If `object` looks like `<inner>.asFile().properties`, return the `<inner>`
/// AST node so the caller can compile a cross-row property lookup.
fn match_as_file_properties(object: &AstExpr) -> Option<&AstExpr> {
    let ExprKind::Member {
        object: inner,
        field: properties_field,
    } = &object.kind
    else {
        return None;
    };
    if properties_field.as_str() != "properties" {
        return None;
    }
    let ExprKind::Call { callee, args } = &inner.kind else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let ExprKind::Member {
        object: target,
        field: method,
    } = &callee.kind
    else {
        return None;
    };
    if method.as_str() != "asFile" {
        return None;
    }
    Some(target.as_ref())
}

/// Compile `<link_expr>.asFile().properties.<field>` into a polars expression.
/// We strip the `[[...]]` wrapper from the link string, then use
/// `replace_strict` with literal lookup Series sourced from the vault
/// DataFrame (file_name as keys, the requested column as values).
fn translate_cross_file_property(
    link_expr: &AstExpr,
    field: &str,
    ctx: &TranslateCtx,
) -> Result<Translated> {
    let link = translate_inner(link_expr, ctx)?;

    // Resolve the property name to its column (frontmatter columns may have
    // been renamed with the `note_` prefix when the key collided with a
    // reserved column name).
    let column_name = ctx
        .schema
        .resolve_frontmatter(field)
        .map(str::to_string)
        .or_else(|| {
            let candidate = format!("file_{field}");
            ctx.schema.has_column(&candidate).then_some(candidate)
        });
    let Some(column_name) = column_name else {
        return Ok(Translated::new(lit(NULL), InferredType::Null));
    };

    let df = &ctx.schema.df;
    let Ok(keys) = df.column("file_name") else {
        return Ok(Translated::new(lit(NULL), InferredType::Null));
    };
    let Ok(values) = df.column(&column_name) else {
        return Ok(Translated::new(lit(NULL), InferredType::Null));
    };
    // `replace_strict` rejects duplicate keys, but Obsidian vaults often have
    // multiple files sharing a stem (`Bible/Books/Philemon.md` and
    // `Bible/Characters/.../Philemon.md`). Keep the first occurrence per
    // stem; the alphabetic walk over the vault makes this stable across
    // runs.
    let (keys_series, values_series) = match dedup_lookup_pair(
        keys.as_materialized_series(),
        values.as_materialized_series(),
    ) {
        Some(pair) => pair,
        None => return Ok(Translated::new(lit(NULL), InferredType::Null)),
    };
    let return_dtype = values_series.dtype().clone();
    let inferred = InferredType::from_dtype(&return_dtype);

    // Strip `[[...]]` from the link expression so the key match works against
    // raw file stems. `link()` always emits the bracketed form; users can
    // also write `value.asFile()` where `value` is already a bare stem, so
    // we only strip when both brackets are present.
    let raw_link = link.expr.cast(DataType::String);
    let stripped = raw_link
        .clone()
        .str()
        .strip_prefix(lit("[["))
        .str()
        .strip_suffix(lit("]]"));
    // If the link contains a pipe (display-text form `[[Target|Text]]`),
    // keep only the target portion before the pipe.
    let target_only = stripped
        .clone()
        .str()
        .split(lit("|"))
        .list()
        .get(lit(0i64), true);

    Ok(Translated::new(
        target_only.replace_strict(
            lit(keys_series),
            lit(values_series),
            Some(lit(NULL)),
            Some(return_dtype),
        ),
        inferred,
    ))
}

/// Build (keys, values) Series for `replace_strict`, dropping any duplicate
/// key entries (keeping the first occurrence) and filtering out null keys.
/// Returns None when no usable rows remain.
fn dedup_lookup_pair(keys: &Series, values: &Series) -> Option<(Series, Series)> {
    let key_chunked = keys.str().ok()?;
    // Borrowed-key dedup: chunked entries live as long as `key_chunked`, so
    // we sidestep a per-row `String` allocation that the previous owned-key
    // HashSet performed.
    let mut seen: std::collections::HashSet<&str> =
        std::collections::HashSet::with_capacity(keys.len());
    let indices: Vec<u32> = key_chunked
        .into_iter()
        .enumerate()
        .filter_map(|(i, k)| {
            let k = k?;
            seen.insert(k).then_some(i as u32)
        })
        .collect();
    if indices.is_empty() {
        return None;
    }
    let idx = IdxCa::from_vec("".into(), indices);
    let keys = keys.take(&idx).ok()?.with_name("".into());
    let values = values.take(&idx).ok()?.with_name("".into());
    Some((keys, values))
}

fn translate_file_field(field: &str, ctx: &TranslateCtx) -> Result<Translated> {
    // Obsidian Bases treats `file.basename` as a synonym for `file.name`
    // (both yield the file stem with no extension).
    let effective = if field == "basename" { "name" } else { field };
    let column_name = format!("file_{effective}");
    match ctx.schema.dtype(&column_name) {
        Some(dt) => Ok(Translated::new(
            col(column_name),
            InferredType::from_dtype(dt),
        )),
        None => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

fn translate_note_field(field: &str, ctx: &TranslateCtx) -> Result<Translated> {
    let resolved = ctx
        .schema
        .resolve_frontmatter(field)
        .map(str::to_string)
        .filter(|name| ctx.schema.has_column(name));
    match resolved {
        Some(name) => {
            let dt = ctx.schema.dtype(&name).cloned().unwrap_or(DataType::Null);
            Ok(Translated::new(col(name), InferredType::from_dtype(&dt)))
        }
        None => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

fn member_on_value(receiver: Translated, field: &str) -> Result<Translated> {
    match (receiver.ty.clone(), field) {
        (InferredType::String, "length") => Ok(Translated::new(
            receiver.expr.str().len_chars().cast(DataType::Int64),
            InferredType::Int,
        )),
        (InferredType::List, "length") => Ok(Translated::new(
            receiver.expr.list().len().cast(DataType::Int64),
            InferredType::Int,
        )),
        (InferredType::Duration, "days") => Ok(Translated::new(
            receiver.expr.dt().total_days(),
            InferredType::Int,
        )),
        (InferredType::Int, "days") | (InferredType::Float, "days") => Ok(Translated::new(
            (receiver.expr / lit(86_400_000_i64)).cast(DataType::Int64),
            InferredType::Int,
        )),
        (ty, "year") if ty.is_date_or_datetime() => Ok(Translated::new(
            receiver.expr.dt().year().cast(DataType::Int64),
            InferredType::Int,
        )),
        (ty, "month") if ty.is_date_or_datetime() => Ok(Translated::new(
            receiver.expr.dt().month().cast(DataType::Int64),
            InferredType::Int,
        )),
        (ty, "day") if ty.is_date_or_datetime() => Ok(Translated::new(
            receiver.expr.dt().day().cast(DataType::Int64),
            InferredType::Int,
        )),
        (ty, "hour") if ty.is_date_or_datetime() => Ok(Translated::new(
            receiver.expr.dt().hour().cast(DataType::Int64),
            InferredType::Int,
        )),
        (ty, "minute") if ty.is_date_or_datetime() => Ok(Translated::new(
            receiver.expr.dt().minute().cast(DataType::Int64),
            InferredType::Int,
        )),
        (ty, "second") if ty.is_date_or_datetime() => Ok(Translated::new(
            receiver.expr.dt().second().cast(DataType::Int64),
            InferredType::Int,
        )),
        (ty, "millisecond") if ty.is_date_or_datetime() => {
            Ok(Translated::new(lit(0i64), InferredType::Int))
        }
        // Fallback: unknown field on unknown type → null
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

// ---------- Index access (a[b]) ----------

fn translate_index(object: &AstExpr, index: &AstExpr, ctx: &TranslateCtx) -> Result<Translated> {
    // formula["name"] and note["name"] — name-with-spaces or kebab-case access.
    if let ExprKind::Variable(obj_name) = &object.kind {
        match obj_name.as_str() {
            "formula" => {
                if let ExprKind::Literal(Literal::Str(name)) = &index.kind {
                    return translate_formula(name, ctx);
                }
                return Ok(Translated::new(lit(NULL), InferredType::Null));
            }
            "note" => {
                if let ExprKind::Literal(Literal::Str(name)) = &index.kind {
                    return translate_note_field(name, ctx);
                }
                return Ok(Translated::new(lit(NULL), InferredType::Null));
            }
            _ => {}
        }
    }
    let recv = translate_inner(object, ctx)?;
    let idx = translate_inner(index, ctx)?;
    match recv.ty {
        InferredType::List => Ok(Translated::new(
            recv.expr.list().get(idx.expr.cast(DataType::Int64), true),
            InferredType::Unknown,
        )),
        InferredType::String => Ok(Translated::new(
            recv.expr
                .str()
                .slice(idx.expr.clone().cast(DataType::Int64), lit(1u32)),
            InferredType::String,
        )),
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

// ---------- Array literal ----------

fn translate_array(items: &[AstExpr], ctx: &TranslateCtx) -> Result<Translated> {
    if items.is_empty() {
        // empty list of strings as a safe default
        return Ok(Translated::new(
            lit(LiteralValue::Series(SpecialEq::new(Series::new(
                "".into(),
                Vec::<String>::new(),
            ))))
            .list()
            .gather(lit(0i64), true)
            .head(Some(0)),
            InferredType::List,
        ));
    }
    let exprs: Vec<Expr> = items
        .iter()
        .map(|i| translate_inner(i, ctx).map(|t| t.expr))
        .collect::<Result<_>>()?;
    Ok(Translated::new(concat_list(exprs)?, InferredType::List))
}

// ---------- Call (function or method) ----------

fn translate_call(callee: &AstExpr, args: &[AstExpr], ctx: &TranslateCtx) -> Result<Translated> {
    match &callee.kind {
        ExprKind::Variable(name) => translate_function_call(name.as_str(), args, ctx),
        ExprKind::Member { object, field } => {
            translate_method_call(object, field.as_str(), args, ctx)
        }
        _ => Err(CrabaseError::ExprEval(format!(
            "Expression is not callable: {callee:?}"
        ))),
    }
}

fn translate_function_call(name: &str, args: &[AstExpr], ctx: &TranslateCtx) -> Result<Translated> {
    match name {
        "if" => {
            let cond = translate_inner(&args[0], ctx)?;
            let then_branch = translate_inner(&args[1], ctx)?;
            let else_branch = translate_inner(&args[2], ctx)?;
            let cond_expr = truthy(cond);
            // Promote both branches to a common type using cast_to_string
            // if their inferred types disagree to avoid polars schema errors.
            let (then_e, else_e, ty) = align_branches(then_branch, else_branch);
            Ok(Translated::new(
                when(cond_expr).then(then_e).otherwise(else_e),
                ty,
            ))
        }
        "number" => {
            let arg = translate_inner(&args[0], ctx)?;
            Ok(Translated::new(
                arg.expr.cast(DataType::Float64),
                InferredType::Float,
            ))
        }
        "list" => {
            // list(x) wraps x in a single-element list. If x is already a list,
            // we keep it as-is. Polars: implode/wrap.
            let exprs = translate_args(args, ctx)?;
            Ok(Translated::new(concat_list(exprs)?, InferredType::List))
        }
        "min" => {
            let exprs = translate_args(args, ctx)?;
            Ok(Translated::new(
                fold_min_max(exprs, true),
                InferredType::Float,
            ))
        }
        "max" => {
            let exprs = translate_args(args, ctx)?;
            Ok(Translated::new(
                fold_min_max(exprs, false),
                InferredType::Float,
            ))
        }
        "date" => translate_date_fn(args, ctx),
        "today" => {
            let today = Local::now().naive_local().date();
            Ok(Translated::new(date_literal(today), InferredType::Date))
        }
        "now" => {
            let now = Local::now().naive_local();
            Ok(Translated::new(
                datetime_literal(now),
                InferredType::Datetime,
            ))
        }
        "link" => {
            let path = translate_inner(&args[0], ctx)?;
            // Idempotent on already-bracketed inputs: `link("[[Foo]]")`
            // returns `"[[Foo]]"`, not `"[[[[Foo]]]]"`. Obsidian's link()
            // collapses a wikilink-shaped string back to itself, which the
            // BibleBookIndex formula (`link(formula.bibleBook)...`) depends on.
            let path_str = path.expr.clone().cast(DataType::String);
            let already_linked = path_str
                .clone()
                .str()
                .starts_with(lit("[["))
                .and(path_str.clone().str().ends_with(lit("]]")));
            if let Some(display_arg) = args.get(1) {
                let display = translate_inner(display_arg, ctx)?;
                let display_str = display.expr.cast(DataType::String);
                let display_present = display_str
                    .clone()
                    .is_not_null()
                    .and(display_str.clone().str().len_chars().gt(lit(0u32)));
                // When the path is already a link string we keep it verbatim
                // even if a display was supplied; mirrors Obsidian's behaviour.
                let with_display = concat_string_exprs(&[
                    lit("[["),
                    path.expr.clone(),
                    lit("|"),
                    display_str,
                    lit("]]"),
                ]);
                let without_display =
                    concat_string_exprs(&[lit("[["), path.expr.clone(), lit("]]")]);
                let new_link = when(display_present)
                    .then(with_display)
                    .otherwise(without_display);
                Ok(Translated::new(
                    when(already_linked).then(path_str).otherwise(new_link),
                    InferredType::String,
                ))
            } else {
                let wrapped = concat_string_exprs(&[lit("[["), path.expr, lit("]]")]);
                Ok(Translated::new(
                    when(already_linked).then(path_str).otherwise(wrapped),
                    InferredType::String,
                ))
            }
        }
        _ => {
            if ctx.formulas.contains_key(name) {
                return translate_formula(name, ctx);
            }
            Ok(Translated::new(lit(NULL), InferredType::Null))
        }
    }
}

fn translate_args(args: &[AstExpr], ctx: &TranslateCtx) -> Result<Vec<Expr>> {
    args.iter()
        .map(|a| translate_inner(a, ctx).map(|t| t.expr))
        .collect()
}

/// Build a min/max expression over an arbitrary list of expressions by
/// folding pairwise `when().then().otherwise()`. Returns `lit(NULL)` for empty.
fn fold_min_max(exprs: Vec<Expr>, want_min: bool) -> Expr {
    let mut iter = exprs.into_iter();
    let Some(first) = iter.next() else {
        return lit(NULL);
    };
    iter.fold(first, |acc, next| {
        let cond = if want_min {
            acc.clone().lt(next.clone())
        } else {
            acc.clone().gt(next.clone())
        };
        when(cond).then(acc).otherwise(next)
    })
}

fn align_branches(a: Translated, b: Translated) -> (Expr, Expr, InferredType) {
    if a.ty == b.ty {
        (a.expr, b.expr, a.ty)
    } else if a.ty == InferredType::Null {
        (a.expr, b.expr, b.ty)
    } else if b.ty == InferredType::Null {
        (a.expr, b.expr, a.ty)
    } else if a.ty.is_numeric() && b.ty.is_numeric() {
        // Promote both to Float64 so an `if(cond, 0, 1.5)`-style expression
        // doesn't collapse to a string and ruin downstream numeric ops.
        (
            a.expr.cast(DataType::Float64),
            b.expr.cast(DataType::Float64),
            InferredType::Float,
        )
    } else {
        // Different concrete types — cast both to string for a safe common dtype.
        (
            a.expr.cast(DataType::String),
            b.expr.cast(DataType::String),
            InferredType::String,
        )
    }
}

fn translate_date_fn(args: &[AstExpr], ctx: &TranslateCtx) -> Result<Translated> {
    let Some(first) = args.first() else {
        return Ok(Translated::new(lit(NULL), InferredType::Null));
    };
    // Literal string: parse at compile time so the result is a typed Date.
    if let ExprKind::Literal(Literal::Str(s)) = &first.kind {
        let stripped = strip_wikilink(s);
        if let Ok(dt) = NaiveDateTime::parse_from_str(stripped, "%Y-%m-%d %H:%M:%S") {
            return Ok(Translated::new(
                datetime_literal(dt),
                InferredType::Datetime,
            ));
        }
        if let Ok(dt) = NaiveDateTime::parse_from_str(stripped, "%Y-%m-%dT%H:%M:%S") {
            return Ok(Translated::new(
                datetime_literal(dt),
                InferredType::Datetime,
            ));
        }
        if let Ok(d) = NaiveDate::parse_from_str(stripped, "%Y-%m-%d") {
            return Ok(Translated::new(date_literal(d), InferredType::Date));
        }
        return Err(CrabaseError::ExprEval(format!("Cannot parse date: {s:?}")));
    }
    // Column input: cast/parse at runtime. Try datetime first, then plain
    // date, then space-separated datetime — Obsidian's `date()` accepts all
    // three shapes, and choosing the wrong one yields null silently.
    let inner = translate_inner(first, ctx)?;
    if matches!(inner.ty, InferredType::Date | InferredType::Datetime) {
        return Ok(Translated::new(inner.expr, inner.ty));
    }
    // For non-string inputs (e.g. an unsupported chain that collapsed to null),
    // return a column-shaped null instead of trying to invoke string methods.
    if !matches!(inner.ty, InferredType::String | InferredType::Unknown) {
        return Ok(Translated::new(
            lit(NULL).cast(DataType::Datetime(TimeUnit::Microseconds, None)),
            InferredType::Datetime,
        ));
    }
    let stripped = inner
        .expr
        .clone()
        .cast(DataType::String)
        .str()
        .replace_all(lit("[["), lit(""), true)
        .str()
        .replace_all(lit("]]"), lit(""), true);

    // Try ISO datetime, ISO date, then space-separated datetime; first match wins.
    let try_iso_dt = stripped.clone().str().strptime(
        DataType::Datetime(TimeUnit::Microseconds, None),
        StrptimeOptions {
            format: Some("%Y-%m-%dT%H:%M:%S".into()),
            strict: false,
            exact: true,
            cache: true,
        },
        lit("raise"),
    );
    let try_iso_date = stripped.clone().str().strptime(
        DataType::Datetime(TimeUnit::Microseconds, None),
        StrptimeOptions {
            format: Some("%Y-%m-%d".into()),
            strict: false,
            exact: true,
            cache: true,
        },
        lit("raise"),
    );
    let try_space_dt = stripped.str().strptime(
        DataType::Datetime(TimeUnit::Microseconds, None),
        StrptimeOptions {
            format: Some("%Y-%m-%d %H:%M:%S".into()),
            strict: false,
            exact: true,
            cache: true,
        },
        lit("raise"),
    );

    let expr = when(try_iso_dt.clone().is_not_null())
        .then(try_iso_dt)
        .when(try_iso_date.clone().is_not_null())
        .then(try_iso_date)
        .otherwise(try_space_dt);

    Ok(Translated::new(expr, InferredType::Datetime))
}

// ---------- Method calls (a.b(...)) ----------

fn translate_method_call(
    object: &AstExpr,
    method: &str,
    args: &[AstExpr],
    ctx: &TranslateCtx,
) -> Result<Translated> {
    // file.<method>(...) is special — they take the schema, not just a column.
    if let ExprKind::Variable(obj_name) = &object.kind {
        if obj_name.as_str() == "file" {
            return translate_file_method(method, args, ctx);
        }
    }

    // `.map()` requires special handling to bind `value`.
    if method == "map" {
        let recv = translate_inner(object, ctx)?;
        let callback = args.first().ok_or_else(|| {
            CrabaseError::ExprEval(".map() requires a callback argument".to_string())
        })?;
        let inner_ctx = ctx.with_value_binding();
        let cb = translate_inner(callback, &inner_ctx)?;
        return Ok(Translated::new(
            recv.expr.list().eval(cb.expr.cast(DataType::String), true),
            InferredType::List,
        ));
    }

    let recv = translate_inner(object, ctx)?;
    let arg_exprs = translate_args(args, ctx)?;
    apply_method(recv, method, args, &arg_exprs, ctx)
}

fn apply_method(
    recv: Translated,
    method: &str,
    raw_args: &[AstExpr],
    arg_exprs: &[Expr],
    ctx: &TranslateCtx,
) -> Result<Translated> {
    // Methods that don't depend on receiver type.
    match method {
        "toString" => {
            return Ok(Translated::new(
                recv.expr.cast(DataType::String),
                InferredType::String,
            ));
        }
        // isEmpty has uniform null-handling across all types: null is empty.
        // String/List additionally check zero length. This matches Obsidian
        // Bases semantics where a missing/null property is considered empty.
        "isEmpty" => {
            let null_check = recv.expr.clone().is_null();
            let expr = match &recv.ty {
                InferredType::String => null_check.or(recv.expr.str().len_chars().eq(lit(0u32))),
                InferredType::List => null_check.or(recv.expr.list().len().eq(lit(0u32))),
                InferredType::Null => lit(true),
                _ => null_check,
            };
            return Ok(Translated::new(expr, InferredType::Bool));
        }
        "isType" => {
            let target = string_literal_arg(raw_args, 0)?;
            let actual = match recv.ty {
                InferredType::Bool => "boolean",
                InferredType::Int | InferredType::Float => "number",
                InferredType::String => "string",
                InferredType::Date | InferredType::Datetime => "date",
                InferredType::List => "list",
                _ => "",
            };
            return Ok(Translated::new(
                lit(actual == target.as_str()),
                InferredType::Bool,
            ));
        }
        "isTruthy" => {
            return Ok(Translated::new(truthy(recv.clone()), InferredType::Bool));
        }
        _ => {}
    }

    match recv.ty.clone() {
        InferredType::String => apply_string_method(recv, method, raw_args, arg_exprs),
        InferredType::Int | InferredType::Float => apply_number_method(recv, method, arg_exprs),
        InferredType::Bool => apply_bool_method(recv, method),
        InferredType::List => apply_list_method(recv, method, arg_exprs),
        InferredType::Date | InferredType::Datetime => {
            apply_date_method(recv, method, raw_args, arg_exprs, ctx)
        }
        InferredType::Null => Ok(Translated::new(lit(NULL), InferredType::Null)),
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

fn apply_string_method(
    recv: Translated,
    method: &str,
    raw_args: &[AstExpr],
    args: &[Expr],
) -> Result<Translated> {
    match method {
        "lower" => Ok(Translated::new(
            recv.expr.str().to_lowercase(),
            InferredType::String,
        )),
        "upper" => Ok(Translated::new(
            recv.expr.str().to_uppercase(),
            InferredType::String,
        )),
        "trim" => Ok(Translated::new(
            recv.expr.str().strip_chars(lit(NULL)),
            InferredType::String,
        )),
        "reverse" => Ok(Translated::new(
            recv.expr.str().reverse(),
            InferredType::String,
        )),
        "title" => {
            // `.title()` strips wikilink wrappers from the receiver (so a
            // property like `study: "[[Foo Bar]]"` titles the visible text,
            // not the markup) before title-casing each word. Polars only
            // exposes `to_titlecase` behind its `nightly` feature, so we
            // route through `map_batches` and titlecase in Rust.
            let stripped = recv
                .expr
                .str()
                .replace_all(lit("[["), lit(""), true)
                .str()
                .replace_all(lit("]]"), lit(""), true);
            let titlecased = stripped.map(
                |s: Column| {
                    let chunked = s.str()?;
                    let out: StringChunked = chunked
                        .into_iter()
                        .map(|opt| opt.map(titlecase_str))
                        .collect();
                    Ok(Some(out.into_column()))
                },
                GetOutput::from_type(DataType::String),
            );
            Ok(Translated::new(titlecased, InferredType::String))
        }
        // Obsidian's string predicates fold case before matching. Implement
        // by lowercasing both sides; polars's `contains_literal` and friends
        // are byte-exact.
        "contains" => Ok(Translated::new(
            recv.expr
                .str()
                .to_lowercase()
                .str()
                .contains_literal(args[0].clone().str().to_lowercase()),
            InferredType::Bool,
        )),
        "containsAny" => {
            // Lowercase the receiver once; polars can fold the repeated clones
            // but keeping it explicit also keeps the generated expression tree
            // smaller.
            let recv_lower = recv.expr.str().to_lowercase();
            let predicate = args.iter().fold(lit(false), |acc, arg| {
                acc.or(recv_lower
                    .clone()
                    .str()
                    .contains_literal(arg.clone().str().to_lowercase()))
            });
            Ok(Translated::new(predicate, InferredType::Bool))
        }
        "startsWith" => Ok(Translated::new(
            recv.expr
                .str()
                .to_lowercase()
                .str()
                .starts_with(args[0].clone().str().to_lowercase()),
            InferredType::Bool,
        )),
        "endsWith" => Ok(Translated::new(
            recv.expr
                .str()
                .to_lowercase()
                .str()
                .ends_with(args[0].clone().str().to_lowercase()),
            InferredType::Bool,
        )),
        "length" => Ok(Translated::new(
            recv.expr.str().len_chars().cast(DataType::Int64),
            InferredType::Int,
        )),
        "replace" => {
            // When the first argument is a regex literal, treat the pattern as
            // a true regex. Otherwise fall back to literal-string replacement,
            // matching the JS/Obsidian behavior.
            let pattern_is_regex = matches!(
                raw_args.first().map(|a| &a.kind),
                Some(ExprKind::Literal(Literal::Regex(_)))
            );
            let expr = if pattern_is_regex {
                recv.expr
                    .str()
                    .replace_all(args[0].clone(), args[1].clone(), false)
            } else {
                recv.expr
                    .str()
                    .replace_all(args[0].clone(), args[1].clone(), true)
            };
            Ok(Translated::new(expr, InferredType::String))
        }
        "split" => Ok(Translated::new(
            recv.expr.str().split(args[0].clone()),
            InferredType::List,
        )),
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

fn apply_number_method(recv: Translated, method: &str, args: &[Expr]) -> Result<Translated> {
    match method {
        "abs" => Ok(Translated::new(recv.expr.abs(), recv.ty)),
        "ceil" => Ok(Translated::new(recv.expr.ceil(), recv.ty)),
        "floor" => Ok(Translated::new(recv.expr.floor(), recv.ty)),
        "round" => {
            let digits = args.first().cloned().unwrap_or(lit(0i64));
            Ok(Translated::new(
                recv.expr.round(digits_to_u32(digits)),
                recv.ty,
            ))
        }
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

fn digits_to_u32(_e: Expr) -> u32 {
    // round() in polars takes a static u32 — we don't have it dynamically.
    // Default to 0; complex digits in formulas not supported.
    0
}

fn apply_bool_method(_recv: Translated, _method: &str) -> Result<Translated> {
    Ok(Translated::new(lit(NULL), InferredType::Null))
}

fn apply_list_method(recv: Translated, method: &str, args: &[Expr]) -> Result<Translated> {
    match method {
        "contains" => Ok(Translated::new(
            recv.expr.list().contains(args[0].clone()),
            InferredType::Bool,
        )),
        "length" => {
            // Propagate null: `null.list.length` is null, not 0. Otherwise
            // expressions like `attendees.length + 1` become `1` for rows
            // without an `attendees` property — wrong by Obsidian semantics.
            let list_expr = recv.expr;
            Ok(Translated::new(
                when(list_expr.clone().is_null())
                    .then(lit(NULL))
                    .otherwise(list_expr.list().len().cast(DataType::Int64)),
                InferredType::Int,
            ))
        }
        "join" => Ok(Translated::new(
            recv.expr
                .list()
                .join(args.first().cloned().unwrap_or(lit(", ")), true),
            InferredType::String,
        )),
        "reverse" => Ok(Translated::new(
            recv.expr.list().reverse(),
            InferredType::List,
        )),
        "unique" => Ok(Translated::new(
            recv.expr.list().unique(),
            InferredType::List,
        )),
        "sort" => Ok(Translated::new(
            recv.expr.list().sort(SortOptions::default()),
            InferredType::List,
        )),
        "slice" => {
            // JS-style `slice(start, end)` on a list. Negative indices count
            // from the end; `slice(0, -1)` drops the last element. polars's
            // list.slice takes (offset, length), so we compute both from the
            // optional start/end args.
            let list_len = recv.expr.clone().list().len().cast(DataType::Int64);
            let start_arg = args.first().cloned().unwrap_or(lit(0i64));
            let end_arg = args.get(1).cloned();
            let normalize = |idx: Expr, len: Expr| -> Expr {
                let idx_i = idx.cast(DataType::Int64);
                when(idx_i.clone().lt(lit(0i64)))
                    .then(len.clone() + idx_i.clone())
                    .otherwise(idx_i)
            };
            let start = normalize(start_arg, list_len.clone());
            let end = match end_arg {
                Some(e) => normalize(e, list_len.clone()),
                None => list_len.clone(),
            };
            // Clamp into [0, list_len] to mirror JS's tolerant behaviour.
            let clamp = |e: Expr, lo: Expr, hi: Expr| {
                when(e.clone().lt(lo.clone()))
                    .then(lo)
                    .otherwise(when(e.clone().gt(hi.clone())).then(hi).otherwise(e))
            };
            let start = clamp(start, lit(0i64), list_len.clone());
            let end = clamp(end, lit(0i64), list_len.clone());
            let length = (end - start.clone()).cast(DataType::Int64);
            // polars list.slice wants non-negative length; clamp negatives to 0.
            let length = when(length.clone().lt(lit(0i64)))
                .then(lit(0i64))
                .otherwise(length);
            Ok(Translated::new(
                recv.expr.list().slice(start, length),
                InferredType::List,
            ))
        }
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

fn apply_date_method(
    recv: Translated,
    method: &str,
    raw_args: &[AstExpr],
    _arg_exprs: &[Expr],
    _ctx: &TranslateCtx,
) -> Result<Translated> {
    match method {
        "format" => {
            let fmt = raw_args
                .first()
                .and_then(|a| match &a.kind {
                    ExprKind::Literal(Literal::Str(s)) => Some(s.as_str()),
                    _ => None,
                })
                .unwrap_or("%Y-%m-%d %H:%M:%S");
            let chrono_fmt = moment_to_chrono(fmt);
            Ok(Translated::new(
                recv.expr.dt().to_string(&chrono_fmt),
                InferredType::String,
            ))
        }
        "date" => Ok(Translated::new(
            recv.expr.dt().truncate(lit("1d")),
            InferredType::Datetime,
        )),
        "time" => Ok(Translated::new(
            recv.expr.dt().to_string("%H:%M:%S"),
            InferredType::String,
        )),
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

// ---------- file.X(...) methods ----------

fn translate_file_method(method: &str, args: &[AstExpr], ctx: &TranslateCtx) -> Result<Translated> {
    match method {
        "inFolder" => {
            let folder = string_literal_arg(args, 0)?;
            let with_slash = format!("{folder}/");
            Ok(Translated::new(
                col("file_folder")
                    .eq(lit(folder.clone()))
                    .or(col("file_folder")
                        .str()
                        .starts_with(lit(with_slash.clone())))
                    .or(col("file_path").str().starts_with(lit(with_slash))),
                InferredType::Bool,
            ))
        }
        "hasTag" => {
            let predicate = args
                .iter()
                .map(string_literal)
                .try_fold(lit(false), |acc, needle| {
                    let needle = needle?;
                    let prefix = format!("{needle}/");
                    let elem = col("");
                    let one = elem
                        .clone()
                        .eq(lit(needle))
                        .or(elem.str().starts_with(lit(prefix)));
                    Ok::<_, CrabaseError>(acc.or(one))
                })?;
            Ok(Translated::new(
                col("file_tags")
                    .list()
                    .eval(predicate, true)
                    .list()
                    .any()
                    .fill_null(lit(false)),
                InferredType::Bool,
            ))
        }
        "hasLink" => {
            let predicate = args
                .iter()
                .map(string_literal)
                .try_fold(lit(false), |acc, needle| {
                    let needle = needle?;
                    let with_slash = format!("/{needle}");
                    let md = format!("{needle}.md");
                    let with_slash_md = format!("/{needle}.md");
                    let elem = col("");
                    let one = elem
                        .clone()
                        .eq(lit(needle))
                        .or(elem.clone().str().ends_with(lit(with_slash)))
                        .or(elem.clone().eq(lit(md)))
                        .or(elem.str().ends_with(lit(with_slash_md)));
                    Ok::<_, CrabaseError>(acc.or(one))
                })?;
            Ok(Translated::new(
                col("file_links")
                    .list()
                    .eval(predicate, true)
                    .list()
                    .any()
                    .fill_null(lit(false)),
                InferredType::Bool,
            ))
        }
        "hasProperty" => {
            let prop = string_literal_arg(args, 0)?;
            let resolved = ctx
                .schema
                .resolve_frontmatter(&prop)
                .map(str::to_string)
                .filter(|n| ctx.schema.has_column(n));
            match resolved {
                Some(name) => Ok(Translated::new(col(name).is_not_null(), InferredType::Bool)),
                None => Ok(Translated::new(lit(false), InferredType::Bool)),
            }
        }
        "asLink" => {
            if let Some(display_arg) = args.first() {
                let display = translate_inner(display_arg, ctx)?;
                Ok(Translated::new(
                    concat_string_exprs(&[
                        lit("[["),
                        col("file_path"),
                        lit("|"),
                        display.expr,
                        lit("]]"),
                    ]),
                    InferredType::String,
                ))
            } else {
                Ok(Translated::new(
                    concat_string_exprs(&[lit("[["), col("file_path"), lit("]]")]),
                    InferredType::String,
                ))
            }
        }
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

// ---------- Binary operators ----------

fn translate_binary(
    op: &BinOp,
    left: &AstExpr,
    right: &AstExpr,
    ctx: &TranslateCtx,
) -> Result<Translated> {
    // Special: Date {+,-} duration-string-literal
    if matches!(op, BinOp::Add | BinOp::Sub) {
        if let Some(t) = try_translate_date_duration(op, left, right, ctx)? {
            return Ok(t);
        }
    }
    let l = translate_inner(left, ctx)?;
    let r = translate_inner(right, ctx)?;
    match op {
        BinOp::Add => translate_add(l, r),
        BinOp::Sub => translate_sub(l, r),
        BinOp::Mul => translate_arith(l, r, |a, b| a * b),
        BinOp::Div => translate_div(l, r),
        BinOp::Mod => translate_arith(l, r, |a, b| a % b),
        BinOp::Eq => translate_eq(l, r, false),
        BinOp::Ne => translate_eq(l, r, true),
        BinOp::Gt => translate_cmp(l, r, |a, b| a.gt(b)),
        BinOp::Lt => translate_cmp(l, r, |a, b| a.lt(b)),
        BinOp::Ge => translate_cmp(l, r, |a, b| a.gt_eq(b)),
        BinOp::Le => translate_cmp(l, r, |a, b| a.lt_eq(b)),
        BinOp::And => Ok(Translated::new(
            truthy(l).and(truthy(r)),
            InferredType::Bool,
        )),
        BinOp::Or => Ok(Translated::new(truthy(l).or(truthy(r)), InferredType::Bool)),
    }
}

fn translate_add(l: Translated, r: Translated) -> Result<Translated> {
    // Date + duration string
    if l.ty.is_date_or_datetime() {
        if let Some(dur) = duration_from_expr(&r.expr, &r.ty) {
            return Ok(Translated::new(l.expr.dt().offset_by(lit(dur)), l.ty));
        }
    }
    // String concatenation (any side string-typed)
    if matches!(l.ty, InferredType::String) || matches!(r.ty, InferredType::String) {
        return Ok(Translated::new(
            concat_string_exprs(&[l.expr.cast(DataType::String), r.expr.cast(DataType::String)]),
            InferredType::String,
        ));
    }
    // List + List → concatenate. Required by formulas like
    // `(file.backlinks + file.links).map(...)`.
    if matches!(l.ty, InferredType::List) && matches!(r.ty, InferredType::List) {
        return Ok(Translated::new(
            concat_list([l.expr, r.expr])?,
            InferredType::List,
        ));
    }
    // Numeric
    let ty = combined_numeric_type(&l.ty, &r.ty);
    Ok(Translated::new(l.expr + r.expr, ty))
}

fn translate_sub(l: Translated, r: Translated) -> Result<Translated> {
    if l.ty.is_date_or_datetime() && r.ty.is_date_or_datetime() {
        return Ok(Translated::new(l.expr - r.expr, InferredType::Duration));
    }
    if l.ty.is_date_or_datetime() {
        if let Some(dur) = duration_from_expr(&r.expr, &r.ty) {
            // Negate the duration: prepend '-'.
            let negated = format!("-{dur}");
            return Ok(Translated::new(l.expr.dt().offset_by(lit(negated)), l.ty));
        }
    }
    let ty = combined_numeric_type(&l.ty, &r.ty);
    Ok(Translated::new(l.expr - r.expr, ty))
}

fn translate_arith(
    l: Translated,
    r: Translated,
    f: impl FnOnce(Expr, Expr) -> Expr,
) -> Result<Translated> {
    let ty = combined_numeric_type(&l.ty, &r.ty);
    Ok(Translated::new(f(l.expr, r.expr), ty))
}

/// Division always yields a Float64 in the expression language — matches the
/// behavior of Obsidian Bases, where `5/2` is `2.5`, not `2`.
fn translate_div(l: Translated, r: Translated) -> Result<Translated> {
    let l_expr = l.expr.cast(DataType::Float64);
    let r_expr = r.expr.cast(DataType::Float64);
    Ok(Translated::new(l_expr / r_expr, InferredType::Float))
}

fn combined_numeric_type(l: &InferredType, r: &InferredType) -> InferredType {
    if matches!(l, InferredType::Float) || matches!(r, InferredType::Float) {
        InferredType::Float
    } else if l.is_numeric() && r.is_numeric() {
        InferredType::Int
    } else {
        InferredType::Float
    }
}

fn translate_eq(l: Translated, r: Translated, neg: bool) -> Result<Translated> {
    // value == null / value != null — polars NULL == NULL is NULL, but the
    // expression language treats null comparison as a definite is_null / is_not_null.
    if matches!(r.ty, InferredType::Null) {
        return Ok(Translated::new(
            if neg {
                l.expr.is_not_null()
            } else {
                l.expr.is_null()
            },
            InferredType::Bool,
        ));
    }
    if matches!(l.ty, InferredType::Null) {
        return Ok(Translated::new(
            if neg {
                r.expr.is_not_null()
            } else {
                r.expr.is_null()
            },
            InferredType::Bool,
        ));
    }
    let (l_expr, r_expr) = cast_for_comparison(l, r);
    // Obsidian semantics: null is treated as "not equal" to any concrete value.
    // Polars returns null when either side is null, so fill_null with the
    // outcome that says "this row's value isn't the one you're asking about".
    let expr = if neg {
        l_expr.neq(r_expr).fill_null(lit(true))
    } else {
        l_expr.eq(r_expr).fill_null(lit(false))
    };
    Ok(Translated::new(expr, InferredType::Bool))
}

fn translate_cmp(
    l: Translated,
    r: Translated,
    f: impl FnOnce(Expr, Expr) -> Expr,
) -> Result<Translated> {
    let (l_expr, r_expr) = cast_for_comparison(l, r);
    Ok(Translated::new(f(l_expr, r_expr), InferredType::Bool))
}

fn cast_for_comparison(l: Translated, r: Translated) -> (Expr, Expr) {
    // If types match, no cast needed.
    if l.ty == r.ty {
        return (l.expr, r.expr);
    }
    // Numeric cross-type: cast to Float64.
    if l.ty.is_numeric() && r.ty.is_numeric() {
        return (
            l.expr.cast(DataType::Float64),
            r.expr.cast(DataType::Float64),
        );
    }
    // Date vs Datetime: cast Date to Datetime
    if l.ty.is_date_or_datetime() && r.ty.is_date_or_datetime() {
        return (
            l.expr
                .cast(DataType::Datetime(TimeUnit::Microseconds, None)),
            r.expr
                .cast(DataType::Datetime(TimeUnit::Microseconds, None)),
        );
    }
    // Default: cast both to String.
    (l.expr.cast(DataType::String), r.expr.cast(DataType::String))
}

// ---------- Unary ----------

fn translate_unary(op: &UnaryOp, operand: &AstExpr, ctx: &TranslateCtx) -> Result<Translated> {
    let inner = translate_inner(operand, ctx)?;
    match op {
        UnaryOp::Not => Ok(Translated::new(truthy(inner).not(), InferredType::Bool)),
        UnaryOp::Neg => Ok(Translated::new(lit(0i64) - inner.expr, inner.ty)),
    }
}

// ---------- Helpers ----------

/// Convert any translated value to a boolean expression matching the current
/// `is_truthy()` semantics (null→false, 0→false, ""→false, []→false, dates→true).
pub fn truthy(t: Translated) -> Expr {
    match t.ty {
        InferredType::Bool => t.expr.fill_null(lit(false)),
        InferredType::Int | InferredType::Float => t
            .expr
            .cast(DataType::Float64)
            .neq(lit(0.0))
            .fill_null(lit(false)),
        InferredType::String => t.expr.str().len_chars().gt(lit(0u32)).fill_null(lit(false)),
        InferredType::List => t.expr.list().len().gt(lit(0u32)).fill_null(lit(false)),
        InferredType::Date | InferredType::Datetime => t.expr.is_not_null(),
        InferredType::Null => lit(false),
        InferredType::Duration => t.expr.is_not_null(),
        InferredType::Unknown => {
            // Best effort: cast to bool, fill null with false.
            t.expr.cast(DataType::Boolean).fill_null(lit(false))
        }
    }
}

fn duration_from_expr(_expr: &Expr, ty: &InferredType) -> Option<String> {
    let _ = ty;
    None
}

/// Detect AST shape `date_expr {+,-} "1d"` (string literal that parses as a
/// Moment.js-style duration) and translate to `dt.offset_by`.
fn try_translate_date_duration(
    op: &BinOp,
    left: &AstExpr,
    right: &AstExpr,
    ctx: &TranslateCtx,
) -> Result<Option<Translated>> {
    let ExprKind::Literal(Literal::Str(dur_str)) = &right.kind else {
        return Ok(None);
    };
    let Some(polars_dur) = moment_duration_to_polars(dur_str) else {
        return Ok(None);
    };
    let lhs = translate_inner(left, ctx)?;
    if !lhs.ty.is_date_or_datetime() {
        return Ok(None);
    }
    let signed = match op {
        BinOp::Add => polars_dur,
        BinOp::Sub => format!("-{polars_dur}"),
        _ => return Ok(None),
    };
    Ok(Some(Translated::new(
        lhs.expr.dt().offset_by(lit(signed)),
        lhs.ty,
    )))
}

/// Translate a Moment.js-style duration string ("1d", "2w", "3M", "1y", "5h",
/// "6m", "7s") to polars duration string syntax. Returns `None` if the input
/// doesn't look like a duration.
fn moment_duration_to_polars(s: &str) -> Option<String> {
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    if split == 0 {
        return None;
    }
    let (num, unit) = s.split_at(split);
    let _: i64 = num.parse().ok()?;
    let pol_unit = match unit {
        "d" | "day" | "days" => "d",
        "w" | "week" | "weeks" => "w",
        "M" | "month" | "months" => "mo",
        "y" | "year" | "years" => "y",
        "h" | "hour" | "hours" => "h",
        "m" | "minute" | "minutes" => "m",
        "s" | "second" | "seconds" => "s",
        _ => return None,
    };
    Some(format!("{num}{pol_unit}"))
}

/// Build a polars literal date from a `NaiveDate`.
fn date_literal(d: NaiveDate) -> Expr {
    let days = (d - NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()).num_days() as i32;
    lit(days).cast(DataType::Date)
}

/// Build a polars literal datetime from a `NaiveDateTime`.
fn datetime_literal(dt: NaiveDateTime) -> Expr {
    let micros = dt.and_utc().timestamp_micros();
    lit(micros).cast(DataType::Datetime(TimeUnit::Microseconds, None))
}

fn concat_string_exprs(parts: &[Expr]) -> Expr {
    concat_str(parts, "", true)
}

fn string_literal(ast: &AstExpr) -> Result<String> {
    match &ast.kind {
        ExprKind::Literal(Literal::Str(s)) => Ok(s.clone()),
        _ => Err(CrabaseError::ExprEval(
            "expected a string literal argument".to_string(),
        )),
    }
}

fn string_literal_arg(args: &[AstExpr], i: usize) -> Result<String> {
    let Some(a) = args.get(i) else {
        return Err(CrabaseError::ExprEval(format!(
            "missing string literal argument at position {i}"
        )));
    };
    string_literal(a)
}

fn strip_wikilink(s: &str) -> &str {
    let t = s.trim();
    let no_open = t.strip_prefix("[[").unwrap_or(t);
    no_open.strip_suffix("]]").unwrap_or(no_open)
}

/// Title-case a string: first char of every word becomes uppercase, the rest
/// lowercase. Word boundaries are runs of non-alphabetic characters (space,
/// dash, digits — so "25-19 to" titles as "25-19 To"; "fg" titles as "Fg").
fn titlecase_str(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut at_word_start = true;
    for c in input.chars() {
        if c.is_alphabetic() {
            if at_word_start {
                for u in c.to_uppercase() {
                    out.push(u);
                }
                at_word_start = false;
            } else {
                for l in c.to_lowercase() {
                    out.push(l);
                }
            }
        } else {
            out.push(c);
            at_word_start = true;
        }
    }
    out
}

/// Convert Moment.js format tokens to chrono strftime specifiers.
pub fn moment_to_chrono(fmt: &str) -> String {
    fmt.replace("YYYY", "%Y")
        .replace("YY", "%y")
        .replace("MM", "%m")
        .replace("DD", "%d")
        .replace("HH", "%H")
        .replace("mm", "%M")
        .replace("ss", "%S")
}
