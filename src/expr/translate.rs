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

    let receiver = translate_inner(object, ctx)?;
    member_on_value(receiver, field)
}

fn translate_file_field(field: &str, ctx: &TranslateCtx) -> Result<Translated> {
    let column_name = format!("file_{field}");
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
    // formula["name"]
    if let ExprKind::Variable(obj_name) = &object.kind {
        if obj_name.as_str() == "formula" {
            if let ExprKind::Literal(Literal::Str(name)) = &index.kind {
                return translate_formula(name, ctx);
            }
            return Ok(Translated::new(lit(NULL), InferredType::Null));
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
            lit(LiteralValue::Series(SpecialEq::new(
                Series::new("".into(), Vec::<String>::new()),
            )))
            .list()
            .gather(lit(0i64), true)
            .head(Some(0)),
            InferredType::List,
        ));
    }
    let translated: Vec<Translated> = items
        .iter()
        .map(|i| translate_inner(i, ctx))
        .collect::<Result<Vec<_>>>()?;
    let exprs: Vec<Expr> = translated.into_iter().map(|t| t.expr).collect();
    Ok(Translated::new(
        concat_list(exprs)?,
        InferredType::List,
    ))
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

fn translate_function_call(
    name: &str,
    args: &[AstExpr],
    ctx: &TranslateCtx,
) -> Result<Translated> {
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
            let inner: Vec<Translated> = args
                .iter()
                .map(|a| translate_inner(a, ctx))
                .collect::<Result<Vec<_>>>()?;
            let exprs: Vec<Expr> = inner.into_iter().map(|t| t.expr).collect();
            Ok(Translated::new(concat_list(exprs)?, InferredType::List))
        }
        "min" => {
            let exprs = translate_args(args, ctx)?;
            Ok(Translated::new(fold_min_max(exprs, true), InferredType::Float))
        }
        "max" => {
            let exprs = translate_args(args, ctx)?;
            Ok(Translated::new(fold_min_max(exprs, false), InferredType::Float))
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
            if let Some(display_arg) = args.get(1) {
                let display = translate_inner(display_arg, ctx)?;
                Ok(Translated::new(
                    concat_string_exprs(&[
                        lit("[["),
                        path.expr,
                        lit("|"),
                        display.expr,
                        lit("]]"),
                    ]),
                    InferredType::String,
                ))
            } else {
                Ok(Translated::new(
                    concat_string_exprs(&[lit("[["), path.expr, lit("]]")]),
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
            return Ok(Translated::new(datetime_literal(dt), InferredType::Datetime));
        }
        if let Ok(dt) = NaiveDateTime::parse_from_str(stripped, "%Y-%m-%dT%H:%M:%S") {
            return Ok(Translated::new(datetime_literal(dt), InferredType::Datetime));
        }
        if let Ok(d) = NaiveDate::parse_from_str(stripped, "%Y-%m-%d") {
            return Ok(Translated::new(date_literal(d), InferredType::Date));
        }
        return Err(CrabaseError::ExprEval(format!("Cannot parse date: {s:?}")));
    }
    // Column input: cast/parse at runtime.
    let inner = translate_inner(first, ctx)?;
    let stripped = inner
        .expr
        .clone()
        .str()
        .replace_all(lit("[["), lit(""), true)
        .str()
        .replace_all(lit("]]"), lit(""), true);
    Ok(Translated::new(
        stripped.str().strptime(
            DataType::Datetime(TimeUnit::Microseconds, None),
            StrptimeOptions {
                format: Some("%Y-%m-%d %H:%M:%S".into()),
                strict: false,
                exact: true,
                cache: true,
            },
            lit("raise"),
        ),
        InferredType::Datetime,
    ))
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
        InferredType::String => apply_string_method(recv, method, arg_exprs),
        InferredType::Int | InferredType::Float => apply_number_method(recv, method, arg_exprs),
        InferredType::Bool => apply_bool_method(recv, method),
        InferredType::List => apply_list_method(recv, method, arg_exprs),
        InferredType::Date | InferredType::Datetime => {
            apply_date_method(recv, method, raw_args, arg_exprs, ctx)
        }
        InferredType::Null => match method {
            "isEmpty" => Ok(Translated::new(lit(true), InferredType::Bool)),
            _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
        },
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

fn apply_string_method(
    recv: Translated,
    method: &str,
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
        "contains" => Ok(Translated::new(
            recv.expr.str().contains_literal(args[0].clone()),
            InferredType::Bool,
        )),
        "startsWith" => Ok(Translated::new(
            recv.expr.str().starts_with(args[0].clone()),
            InferredType::Bool,
        )),
        "endsWith" => Ok(Translated::new(
            recv.expr.str().ends_with(args[0].clone()),
            InferredType::Bool,
        )),
        "isEmpty" => Ok(Translated::new(
            recv.expr.str().len_chars().eq(lit(0u32)),
            InferredType::Bool,
        )),
        "length" => Ok(Translated::new(
            recv.expr.str().len_chars().cast(DataType::Int64),
            InferredType::Int,
        )),
        "replace" => Ok(Translated::new(
            recv.expr
                .str()
                .replace_all(args[0].clone(), args[1].clone(), true),
            InferredType::String,
        )),
        "split" => Ok(Translated::new(
            recv.expr.str().split(args[0].clone()),
            InferredType::List,
        )),
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

fn apply_number_method(
    recv: Translated,
    method: &str,
    args: &[Expr],
) -> Result<Translated> {
    match method {
        "abs" => Ok(Translated::new(recv.expr.abs(), recv.ty)),
        "ceil" => Ok(Translated::new(recv.expr.ceil(), recv.ty)),
        "floor" => Ok(Translated::new(recv.expr.floor(), recv.ty)),
        "round" => {
            let digits = args
                .first()
                .cloned()
                .unwrap_or(lit(0i64));
            Ok(Translated::new(
                recv.expr.round(digits_to_u32(digits)),
                recv.ty,
            ))
        }
        "isEmpty" => Ok(Translated::new(lit(false), InferredType::Bool)),
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

fn digits_to_u32(_e: Expr) -> u32 {
    // round() in polars takes a static u32 — we don't have it dynamically.
    // Default to 0; complex digits in formulas not supported.
    0
}

fn apply_bool_method(_recv: Translated, method: &str) -> Result<Translated> {
    match method {
        "isEmpty" => Ok(Translated::new(lit(false), InferredType::Bool)),
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

fn apply_list_method(
    recv: Translated,
    method: &str,
    args: &[Expr],
) -> Result<Translated> {
    match method {
        "contains" => Ok(Translated::new(
            recv.expr.list().contains(args[0].clone()),
            InferredType::Bool,
        )),
        "length" => Ok(Translated::new(
            recv.expr.list().len().cast(DataType::Int64),
            InferredType::Int,
        )),
        "isEmpty" => Ok(Translated::new(
            recv.expr.list().len().eq(lit(0u32)),
            InferredType::Bool,
        )),
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
        "isEmpty" => Ok(Translated::new(lit(false), InferredType::Bool)),
        _ => Ok(Translated::new(lit(NULL), InferredType::Null)),
    }
}

// ---------- file.X(...) methods ----------

fn translate_file_method(
    method: &str,
    args: &[AstExpr],
    ctx: &TranslateCtx,
) -> Result<Translated> {
    match method {
        "inFolder" => {
            let folder = string_literal_arg(args, 0)?;
            let with_slash = format!("{folder}/");
            Ok(Translated::new(
                col("file_folder")
                    .eq(lit(folder.clone()))
                    .or(col("file_folder").str().starts_with(lit(with_slash.clone())))
                    .or(col("file_path").str().starts_with(lit(with_slash))),
                InferredType::Bool,
            ))
        }
        "hasTag" => {
            let needles: Vec<String> = args
                .iter()
                .map(string_literal)
                .collect::<Result<Vec<_>>>()?;
            // Translate to col("file_tags").list.eval(any_match).list.any()
            let mut predicate: Expr = lit(false);
            for needle in &needles {
                let prefix = format!("{needle}/");
                let elem = col("");
                let one = elem.clone().eq(lit(needle.clone()))
                    .or(elem.str().starts_with(lit(prefix)));
                predicate = predicate.or(one);
            }
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
            let needles: Vec<String> = args
                .iter()
                .map(string_literal)
                .collect::<Result<Vec<_>>>()?;
            let mut predicate: Expr = lit(false);
            for needle in &needles {
                let with_slash = format!("/{needle}");
                let md = format!("{needle}.md");
                let with_slash_md = format!("/{needle}.md");
                let elem = col("");
                let one = elem.clone().eq(lit(needle.clone()))
                    .or(elem.clone().str().ends_with(lit(with_slash)))
                    .or(elem.clone().eq(lit(md)))
                    .or(elem.str().ends_with(lit(with_slash_md)));
                predicate = predicate.or(one);
            }
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
                Some(name) => Ok(Translated::new(
                    col(name).is_not_null(),
                    InferredType::Bool,
                )),
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
        BinOp::Div => translate_arith(l, r, |a, b| a / b),
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
        BinOp::Or => Ok(Translated::new(
            truthy(l).or(truthy(r)),
            InferredType::Bool,
        )),
    }
}

fn translate_add(l: Translated, r: Translated) -> Result<Translated> {
    // Date + duration string
    if l.ty.is_date_or_datetime() {
        if let Some(dur) = duration_from_expr(&r.expr, &r.ty) {
            return Ok(Translated::new(
                l.expr.dt().offset_by(lit(dur)),
                l.ty,
            ));
        }
    }
    // String concatenation (any side string-typed)
    if matches!(l.ty, InferredType::String) || matches!(r.ty, InferredType::String) {
        return Ok(Translated::new(
            concat_string_exprs(&[
                l.expr.cast(DataType::String),
                r.expr.cast(DataType::String),
            ]),
            InferredType::String,
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
            if neg { l.expr.is_not_null() } else { l.expr.is_null() },
            InferredType::Bool,
        ));
    }
    if matches!(l.ty, InferredType::Null) {
        return Ok(Translated::new(
            if neg { r.expr.is_not_null() } else { r.expr.is_null() },
            InferredType::Bool,
        ));
    }
    let (l_expr, r_expr) = cast_for_comparison(l, r);
    let expr = if neg {
        l_expr.neq(r_expr)
    } else {
        l_expr.eq(r_expr)
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
        return (l.expr.cast(DataType::Float64), r.expr.cast(DataType::Float64));
    }
    // Date vs Datetime: cast Date to Datetime
    if l.ty.is_date_or_datetime() && r.ty.is_date_or_datetime() {
        return (
            l.expr.cast(DataType::Datetime(TimeUnit::Microseconds, None)),
            r.expr.cast(DataType::Datetime(TimeUnit::Microseconds, None)),
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
        InferredType::Int | InferredType::Float => {
            t.expr.cast(DataType::Float64).neq(lit(0.0)).fill_null(lit(false))
        }
        InferredType::String => t.expr.str().len_chars().gt(lit(0u32)).fill_null(lit(false)),
        InferredType::List => t.expr.list().len().gt(lit(0u32)).fill_null(lit(false)),
        InferredType::Date | InferredType::Datetime => {
            t.expr.is_not_null()
        }
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

