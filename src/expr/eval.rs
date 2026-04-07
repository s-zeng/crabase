use crate::error::{CrabaseError, Result};
use crate::expr::ast::{BinOp, Expr, UnaryOp};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, Timelike};
use std::collections::HashMap;

/// Runtime value in the expression evaluator
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    List(Vec<Value>),
    Date(NaiveDateTime),
}

impl Value {
    /// Coerce to boolean (truthiness)
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(v) => !v.is_empty(),
            Value::Date(_) => true,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::Str(_) => "string",
            Value::List(_) => "list",
            Value::Date(_) => "date",
        }
    }

    /// Convert to display string
    pub fn to_display(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => format_number(*n),
            Value::Str(s) => s.clone(),
            Value::List(items) => items
                .iter()
                .map(|v| v.to_display())
                .collect::<Vec<_>>()
                .join(", "),
            Value::Date(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    }

    /// Try to get as a number, returns None if not possible
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Str(s) => s.parse::<f64>().ok(),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Parse a duration string like "1d", "2w", "3M", "4y", "5h", "6m", "7s"
/// Returns (amount, canonical_unit) or None if unparseable.
fn parse_duration_string(s: &str) -> Option<(i64, &'static str)> {
    // Split leading digits from unit suffix
    let split_pos = s.find(|c: char| !c.is_ascii_digit())?;
    let (num_str, unit_str) = s.split_at(split_pos);
    let amount: i64 = num_str.parse().ok()?;
    let unit = match unit_str {
        "d" | "day" | "days" => "days",
        "w" | "week" | "weeks" => "weeks",
        "M" | "month" | "months" => "months",
        "y" | "year" | "years" => "years",
        "h" | "hour" | "hours" => "hours",
        "m" | "minute" | "minutes" => "minutes",
        "s" | "second" | "seconds" => "seconds",
        _ => return None,
    };
    Some((amount, unit))
}

/// Apply a signed duration to a NaiveDateTime.
fn apply_duration(dt: NaiveDateTime, amount: i64, unit: &str) -> Result<NaiveDateTime> {
    match unit {
        "days" => dt
            .checked_add_signed(chrono::Duration::days(amount))
            .ok_or_else(|| CrabaseError::ExprEval("date overflow adding days".to_string())),
        "weeks" => dt
            .checked_add_signed(chrono::Duration::weeks(amount))
            .ok_or_else(|| CrabaseError::ExprEval("date overflow adding weeks".to_string())),
        "hours" => dt
            .checked_add_signed(chrono::Duration::hours(amount))
            .ok_or_else(|| CrabaseError::ExprEval("date overflow adding hours".to_string())),
        "minutes" => dt
            .checked_add_signed(chrono::Duration::minutes(amount))
            .ok_or_else(|| CrabaseError::ExprEval("date overflow adding minutes".to_string())),
        "seconds" => dt
            .checked_add_signed(chrono::Duration::seconds(amount))
            .ok_or_else(|| CrabaseError::ExprEval("date overflow adding seconds".to_string())),
        "months" => {
            if amount >= 0 {
                dt.checked_add_months(chrono::Months::new(amount as u32))
                    .ok_or_else(|| CrabaseError::ExprEval("date overflow adding months".to_string()))
            } else {
                dt.checked_sub_months(chrono::Months::new((-amount) as u32))
                    .ok_or_else(|| CrabaseError::ExprEval("date overflow subtracting months".to_string()))
            }
        }
        "years" => {
            if amount >= 0 {
                dt.checked_add_months(chrono::Months::new((amount as u32) * 12))
                    .ok_or_else(|| CrabaseError::ExprEval("date overflow adding years".to_string()))
            } else {
                dt.checked_sub_months(chrono::Months::new(((-amount) as u32) * 12))
                    .ok_or_else(|| CrabaseError::ExprEval("date overflow subtracting years".to_string()))
            }
        }
        other => Err(CrabaseError::ExprEval(format!("Unknown duration unit: {other}"))),
    }
}

/// Convert Moment.js format tokens to chrono strftime specifiers.
fn moment_to_chrono(fmt: &str) -> String {
    // Order matters: longer tokens first to avoid partial replacements
    fmt.replace("YYYY", "%Y")
        .replace("YY", "%y")
        .replace("MM", "%m")
        .replace("DD", "%d")
        .replace("HH", "%H")
        .replace("mm", "%M")
        .replace("ss", "%S")
}

/// Produce a human-readable relative time string from a duration.
fn format_relative(diff: chrono::Duration) -> String {
    let secs = diff.num_seconds();
    let abs = secs.unsigned_abs();
    let suffix = if secs >= 0 { "ago" } else { "from now" };
    let amount;
    let unit;
    if abs < 60 {
        amount = abs;
        unit = if abs == 1 { "second" } else { "seconds" };
    } else if abs < 3600 {
        amount = abs / 60;
        unit = if amount == 1 { "minute" } else { "minutes" };
    } else if abs < 86400 {
        amount = abs / 3600;
        unit = if amount == 1 { "hour" } else { "hours" };
    } else if abs < 86400 * 30 {
        amount = abs / 86400;
        unit = if amount == 1 { "day" } else { "days" };
    } else if abs < 86400 * 365 {
        amount = abs / (86400 * 30);
        unit = if amount == 1 { "month" } else { "months" };
    } else {
        amount = abs / (86400 * 365);
        unit = if amount == 1 { "year" } else { "years" };
    }
    format!("{amount} {unit} {suffix}")
}

/// Evaluation context for a single file
pub struct EvalContext {
    /// File properties (name, path, folder, ext, size, ctime, mtime, tags, links)
    pub file_props: HashMap<String, Value>,
    /// Frontmatter properties
    pub note_props: HashMap<String, Value>,
    /// Formula definitions (name -> expression string)
    pub formulas: HashMap<String, String>,
}

impl EvalContext {
    pub fn new(
        file_props: HashMap<String, Value>,
        note_props: HashMap<String, Value>,
        formulas: HashMap<String, String>,
    ) -> Self {
        EvalContext {
            file_props,
            note_props,
            formulas,
        }
    }

    fn get_variable(&self, name: &str) -> Value {
        // Try note props first, then file props
        self.note_props.get(name).cloned().unwrap_or(Value::Null)
    }
}

pub fn eval(expr: &Expr, ctx: &EvalContext) -> Result<Value> {
    match expr {
        Expr::Number(n) => Ok(Value::Number(*n)),
        Expr::Str(s) => Ok(Value::Str(s.clone())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Null => Ok(Value::Null),

        Expr::Ident(name) => {
            // Check for formula
            if let Some(formula_expr) = ctx.formulas.get(name) {
                let formula_expr = formula_expr.clone();
                let parsed = crate::expr::parser::parse(&formula_expr)?;
                return eval(&parsed, ctx);
            }
            Ok(ctx.get_variable(name))
        }

        Expr::Member { object, field } => {
            let obj_val = eval_object_access(object, field, ctx)?;
            Ok(obj_val)
        }

        Expr::Index { object, index } => {
            let obj = eval(object, ctx)?;
            let idx = eval(index, ctx)?;
            match (obj, idx) {
                (Value::List(items), Value::Number(n)) => {
                    let i = n as usize;
                    Ok(items.into_iter().nth(i).unwrap_or(Value::Null))
                }
                (Value::Str(s), Value::Number(n)) => {
                    let i = n as usize;
                    Ok(s.chars()
                        .nth(i)
                        .map(|c| Value::Str(c.to_string()))
                        .unwrap_or(Value::Null))
                }
                _ => Ok(Value::Null),
            }
        }

        Expr::Call { callee, args } => match callee.as_ref() {
            Expr::Ident(name) => eval_func_call(name, args, ctx),
            Expr::Member { object, field } => eval_method_call(object, field, args, ctx),
            other => Err(CrabaseError::ExprEval(format!(
                "Expression is not callable: {other:?}"
            ))),
        },

        Expr::Array(items) => {
            let values = items.iter().map(|e| eval(e, ctx)).collect::<Result<Vec<_>>>()?;
            Ok(Value::List(values))
        }

        Expr::BinOp { op, left, right } => eval_binop(op, left, right, ctx),

        Expr::UnaryOp { op, operand } => {
            let val = eval(operand, ctx)?;
            match op {
                UnaryOp::Not => Ok(Value::Bool(!val.is_truthy())),
                UnaryOp::Neg => match val {
                    Value::Number(n) => Ok(Value::Number(-n)),
                    other => Err(CrabaseError::ExprEval(format!("Cannot negate {other:?}"))),
                },
            }
        }
    }
}

fn eval_object_access(object: &Expr, field: &str, ctx: &EvalContext) -> Result<Value> {
    // Special case: top-level "file" object
    if let Expr::Ident(obj_name) = object {
        if obj_name == "file" {
            return Ok(ctx.file_props.get(field).cloned().unwrap_or(Value::Null));
        }
        if obj_name == "note" {
            return Ok(ctx.note_props.get(field).cloned().unwrap_or(Value::Null));
        }
        if obj_name == "formula" {
            if let Some(formula_expr) = ctx.formulas.get(field) {
                let formula_expr = formula_expr.clone();
                let parsed = crate::expr::parser::parse(&formula_expr)?;
                return eval(&parsed, ctx);
            }
            return Ok(Value::Null);
        }
    }

    // Otherwise evaluate the object and access a field on the value
    let obj_val = eval(object, ctx)?;
    match (&obj_val, field) {
        (Value::Str(s), "length") => Ok(Value::Number(s.chars().count() as f64)),
        (Value::List(items), "length") => Ok(Value::Number(items.len() as f64)),
        // Date field access
        (Value::Date(dt), "year") => Ok(Value::Number(dt.year() as f64)),
        (Value::Date(dt), "month") => Ok(Value::Number(dt.month() as f64)),
        (Value::Date(dt), "day") => Ok(Value::Number(dt.day() as f64)),
        (Value::Date(dt), "hour") => Ok(Value::Number(dt.hour() as f64)),
        (Value::Date(dt), "minute") => Ok(Value::Number(dt.minute() as f64)),
        (Value::Date(dt), "second") => Ok(Value::Number(dt.second() as f64)),
        (Value::Date(_), "millisecond") => Ok(Value::Number(0.0)),
        _ => Ok(Value::Null),
    }
}

fn eval_method_call(
    object: &Expr,
    method: &str,
    args: &[Expr],
    ctx: &EvalContext,
) -> Result<Value> {
    // Special case for "file" object methods
    if let Expr::Ident(obj_name) = object {
        if obj_name == "file" {
            let eval_args = args
                .iter()
                .map(|a| eval(a, ctx))
                .collect::<Result<Vec<_>>>()?;
            return eval_file_method(method, &eval_args, ctx);
        }
    }

    let obj_val = eval(object, ctx)?;
    let eval_args = args
        .iter()
        .map(|a| eval(a, ctx))
        .collect::<Result<Vec<_>>>()?;

    match (&obj_val, method) {
        // Date methods
        (Value::Date(dt), "format") => {
            let fmt_str = eval_args
                .first()
                .and_then(|v| if let Value::Str(s) = v { Some(s.as_str()) } else { None })
                .unwrap_or("%Y-%m-%d %H:%M:%S");
            let chrono_fmt = moment_to_chrono(fmt_str);
            Ok(Value::Str(dt.format(&chrono_fmt).to_string()))
        }
        (Value::Date(dt), "date") => {
            let d = dt.date();
            Ok(Value::Date(d.and_hms_opt(0, 0, 0).unwrap_or(*dt)))
        }
        (Value::Date(dt), "time") => Ok(Value::Str(dt.format("%H:%M:%S").to_string())),
        (Value::Date(dt), "relative") => {
            let diff = Local::now().naive_local().signed_duration_since(*dt);
            Ok(Value::Str(format_relative(diff)))
        }
        (Value::Date(_), "isEmpty") => Ok(Value::Bool(false)),
        (Value::Date(_), "isTruthy") => Ok(Value::Bool(true)),
        (Value::Date(dt), "toString") => Ok(Value::Str(dt.format("%Y-%m-%d %H:%M:%S").to_string())),
        (Value::Date(_), "isType") => {
            let type_name = eval_args
                .first()
                .and_then(|v| if let Value::Str(t) = v { Some(t.as_str()) } else { None })
                .unwrap_or("");
            Ok(Value::Bool(type_name == "date"))
        }
        // String methods
        (Value::Str(s), "contains") => {
            let needle = eval_args
                .first()
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            Ok(Value::Bool(s.contains(needle)))
        }
        (Value::Str(s), "startsWith") => {
            let prefix = eval_args
                .first()
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            Ok(Value::Bool(s.starts_with(prefix)))
        }
        (Value::Str(s), "endsWith") => {
            let suffix = eval_args
                .first()
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            Ok(Value::Bool(s.ends_with(suffix)))
        }
        (Value::Str(s), "lower") => Ok(Value::Str(s.to_lowercase())),
        (Value::Str(s), "upper") => Ok(Value::Str(s.to_uppercase())),
        (Value::Str(s), "title") => {
            let titled = s
                .split_whitespace()
                .map(|w| {
                    let mut chars = w.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().to_string() + chars.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            Ok(Value::Str(titled))
        }
        (Value::Str(s), "trim") => Ok(Value::Str(s.trim().to_string())),
        (Value::Str(s), "reverse") => Ok(Value::Str(s.chars().rev().collect())),
        (Value::Str(s), "isEmpty") => Ok(Value::Bool(s.is_empty())),
        (Value::Str(s), "length") => Ok(Value::Number(s.chars().count() as f64)),
        (Value::Str(s), "toString") => Ok(Value::Str(s.clone())),
        (Value::Str(s), "isTruthy") => Ok(Value::Bool(!s.is_empty())),
        (Value::Str(_), "isType") => {
            let type_name = eval_args
                .first()
                .and_then(|v| {
                    if let Value::Str(t) = v {
                        Some(t.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            Ok(Value::Bool(type_name == "string"))
        }
        (Value::Str(s), "slice") => {
            let start = eval_args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as usize;
            let chars: Vec<char> = s.chars().collect();
            let end = eval_args
                .get(1)
                .and_then(|v| v.as_number())
                .map(|n| n as usize)
                .unwrap_or(chars.len());
            let sliced: String = chars[start.min(chars.len())..end.min(chars.len())]
                .iter()
                .collect();
            Ok(Value::Str(sliced))
        }
        (Value::Str(s), "split") => {
            let sep = eval_args
                .first()
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or(",");
            let parts: Vec<Value> = s.split(sep).map(|p| Value::Str(p.to_string())).collect();
            Ok(Value::List(parts))
        }
        (Value::Str(s), "repeat") => {
            let count = eval_args.first().and_then(|v| v.as_number()).unwrap_or(1.0) as usize;
            Ok(Value::Str(s.repeat(count)))
        }
        (Value::Str(s), "replace") => {
            let pattern = eval_args
                .first()
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            let replacement = eval_args
                .get(1)
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            Ok(Value::Str(s.replace(&pattern, replacement)))
        }
        (Value::Str(s), "toFixed") => {
            if let Ok(n) = s.parse::<f64>() {
                let precision =
                    eval_args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as usize;
                Ok(Value::Str(format!("{n:.precision$}")))
            } else {
                Ok(Value::Str(s.clone()))
            }
        }
        // Number methods
        (Value::Number(n), "abs") => Ok(Value::Number(n.abs())),
        (Value::Number(n), "ceil") => Ok(Value::Number(n.ceil())),
        (Value::Number(n), "floor") => Ok(Value::Number(n.floor())),
        (Value::Number(n), "round") => {
            if let Some(digits) = eval_args.first().and_then(|v| v.as_number()) {
                let factor = 10f64.powi(digits as i32);
                Ok(Value::Number((n * factor).round() / factor))
            } else {
                Ok(Value::Number(n.round()))
            }
        }
        (Value::Number(n), "toFixed") => {
            let precision = eval_args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as usize;
            Ok(Value::Str(format!("{n:.precision$}")))
        }
        (Value::Number(_), "isEmpty") => Ok(Value::Bool(false)),
        (Value::Number(n), "toString") => Ok(Value::Str(format_number(*n))),
        (Value::Number(n), "isTruthy") => Ok(Value::Bool(*n != 0.0)),
        (Value::Number(_), "isType") => {
            let type_name = eval_args
                .first()
                .and_then(|v| {
                    if let Value::Str(t) = v {
                        Some(t.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            Ok(Value::Bool(type_name == "number"))
        }
        // Bool methods
        (Value::Bool(b), "isTruthy") => Ok(Value::Bool(*b)),
        (Value::Bool(b), "toString") => Ok(Value::Str(b.to_string())),
        (Value::Bool(_), "isType") => {
            let type_name = eval_args
                .first()
                .and_then(|v| {
                    if let Value::Str(t) = v {
                        Some(t.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            Ok(Value::Bool(type_name == "boolean"))
        }
        // List methods
        (Value::List(items), "contains") => {
            let needle = eval_args.first().cloned().unwrap_or(Value::Null);
            Ok(Value::Bool(
                items.iter().any(|item| values_equal(item, &needle)),
            ))
        }
        (Value::List(items), "containsAll") => {
            let result = eval_args
                .iter()
                .all(|needle| items.iter().any(|item| values_equal(item, needle)));
            Ok(Value::Bool(result))
        }
        (Value::List(items), "containsAny") => {
            let result = eval_args
                .iter()
                .any(|needle| items.iter().any(|item| values_equal(item, needle)));
            Ok(Value::Bool(result))
        }
        (Value::List(items), "length") => Ok(Value::Number(items.len() as f64)),
        (Value::List(items), "isEmpty") => Ok(Value::Bool(items.is_empty())),
        (Value::List(items), "join") => {
            let sep = eval_args
                .first()
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or(", ");
            let joined = items
                .iter()
                .map(|v| v.to_display())
                .collect::<Vec<_>>()
                .join(sep);
            Ok(Value::Str(joined))
        }
        (Value::List(items), "reverse") => Ok(Value::List(items.iter().cloned().rev().collect())),
        (Value::List(items), "sort") => {
            let mut sorted = items.clone();
            sorted.sort_by(compare_values);
            Ok(Value::List(sorted))
        }
        (Value::List(items), "unique") => Ok(Value::List(items.iter().cloned().fold(
            Vec::new(),
            |mut acc, item| {
                if !acc.iter().any(|existing| values_equal(existing, &item)) {
                    acc.push(item);
                }
                acc
            },
        ))),
        (Value::List(items), "flat") => Ok(Value::List(
            items
                .iter()
                .flat_map(|item| match item {
                    Value::List(inner) => inner.clone(),
                    other => vec![other.clone()],
                })
                .collect(),
        )),
        (Value::List(items), "slice") => {
            let start = eval_args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as usize;
            let end = eval_args
                .get(1)
                .and_then(|v| v.as_number())
                .map(|n| n as usize)
                .unwrap_or(items.len());
            Ok(Value::List(
                items[start.min(items.len())..end.min(items.len())].to_vec(),
            ))
        }
        (Value::List(_), "isTruthy") => Ok(Value::Bool(obj_val.is_truthy())),
        (Value::List(_), "isType") => {
            let type_name = eval_args
                .first()
                .and_then(|v| {
                    if let Value::Str(t) = v {
                        Some(t.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            Ok(Value::Bool(type_name == "list"))
        }
        // Null methods
        (Value::Null, "isEmpty") => Ok(Value::Bool(true)),
        (Value::Null, "isTruthy") => Ok(Value::Bool(false)),
        _ => {
            // Unknown method - return Null rather than error for resilience
            Ok(Value::Null)
        }
    }
}

fn eval_file_method(method: &str, args: &[Value], ctx: &EvalContext) -> Result<Value> {
    match method {
        "hasTag" => {
            let tags = ctx.file_props.get("tags").cloned().unwrap_or(Value::Null);
            match tags {
                Value::List(tag_list) => {
                    let result = args.iter().any(|arg| {
                        if let Value::Str(needle) = arg {
                            tag_list.iter().any(|tag| {
                                if let Value::Str(t) = tag {
                                    t == needle || t.starts_with(&format!("{needle}/"))
                                } else {
                                    false
                                }
                            })
                        } else {
                            false
                        }
                    });
                    Ok(Value::Bool(result))
                }
                _ => Ok(Value::Bool(false)),
            }
        }
        "inFolder" => {
            let folder_arg = args
                .first()
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            let file_folder = ctx
                .file_props
                .get("folder")
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            let file_path = ctx
                .file_props
                .get("path")
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");

            let in_folder = file_path.starts_with(&format!("{folder_arg}/"))
                || file_folder == folder_arg
                || file_folder.starts_with(&format!("{folder_arg}/"));

            Ok(Value::Bool(in_folder))
        }
        "hasLink" => {
            let links = ctx.file_props.get("links").cloned().unwrap_or(Value::Null);
            match links {
                Value::List(link_list) => {
                    let result = args.iter().any(|arg| {
                        if let Value::Str(needle) = arg {
                            link_list.iter().any(|link| {
                                if let Value::Str(l) = link {
                                    l == needle
                                        || l.ends_with(&format!("/{needle}"))
                                        || l == &format!("{needle}.md")
                                        || l.ends_with(&format!("/{needle}.md"))
                                } else {
                                    false
                                }
                            })
                        } else {
                            false
                        }
                    });
                    Ok(Value::Bool(result))
                }
                _ => Ok(Value::Bool(false)),
            }
        }
        "hasProperty" => {
            let prop_name = args
                .first()
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            Ok(Value::Bool(ctx.note_props.contains_key(prop_name)))
        }
        "asLink" => {
            let display = args.first().and_then(|v| {
                if let Value::Str(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            });
            let path = ctx
                .file_props
                .get("path")
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            let link = if let Some(d) = display {
                format!("[[{path}|{d}]]")
            } else {
                format!("[[{path}]]")
            };
            Ok(Value::Str(link))
        }
        _ => Ok(Value::Null),
    }
}

fn eval_func_call(name: &str, args: &[Expr], ctx: &EvalContext) -> Result<Value> {
    let eval_args = args
        .iter()
        .map(|a| eval(a, ctx))
        .collect::<Result<Vec<_>>>()?;

    match name {
        "if" => {
            let cond = eval_args.first().cloned().unwrap_or(Value::Null);
            let true_val = eval_args.get(1).cloned().unwrap_or(Value::Null);
            let false_val = eval_args.get(2).cloned().unwrap_or(Value::Null);
            if cond.is_truthy() {
                Ok(true_val)
            } else {
                Ok(false_val)
            }
        }
        "list" => {
            if eval_args.len() == 1 {
                if let Value::List(items) = &eval_args[0] {
                    Ok(Value::List(items.clone()))
                } else {
                    Ok(Value::List(vec![eval_args[0].clone()]))
                }
            } else {
                Ok(Value::List(eval_args))
            }
        }
        "number" => {
            let val = eval_args.first().cloned().unwrap_or(Value::Null);
            match val.as_number() {
                Some(n) => Ok(Value::Number(n)),
                None => Ok(Value::Null),
            }
        }
        "min" => {
            let min = eval_args
                .iter()
                .filter_map(|v| v.as_number())
                .reduce(f64::min);
            Ok(min.map(Value::Number).unwrap_or(Value::Null))
        }
        "max" => {
            let max = eval_args
                .iter()
                .filter_map(|v| v.as_number())
                .reduce(f64::max);
            Ok(max.map(Value::Number).unwrap_or(Value::Null))
        }
        "date" => {
            match eval_args.first() {
                None | Some(Value::Null) => Ok(Value::Null),
                Some(Value::Date(dt)) => Ok(Value::Date(*dt)),
                Some(Value::Str(s)) => {
                    // Try datetime format first, then date-only
                    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
                        Ok(Value::Date(dt))
                    } else if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                        Ok(Value::Date(
                            d.and_hms_opt(0, 0, 0)
                                .ok_or_else(|| CrabaseError::ExprEval(format!("Invalid date: {s}")))?,
                        ))
                    } else {
                        Err(CrabaseError::ExprEval(format!("Cannot parse date: {s:?}")))
                    }
                }
                Some(other) => Err(CrabaseError::ExprEval(format!(
                    "date() expects a string, got {other:?}"
                ))),
            }
        }
        "today" => {
            let today = Local::now()
                .naive_local()
                .date()
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| CrabaseError::ExprEval("Failed to construct today".to_string()))?;
            Ok(Value::Date(today))
        }
        "now" => Ok(Value::Date(Local::now().naive_local())),
        "link" => {
            let path = eval_args
                .first()
                .and_then(|v| {
                    if let Value::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            let display = eval_args.get(1).and_then(|v| {
                if let Value::Str(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            });
            let link = if let Some(d) = display {
                format!("[[{path}|{d}]]")
            } else {
                format!("[[{path}]]")
            };
            Ok(Value::Str(link))
        }
        _ => {
            // Check formulas
            if let Some(formula_expr) = ctx.formulas.get(name) {
                let formula_expr = formula_expr.clone();
                let parsed = crate::expr::parser::parse(&formula_expr)?;
                return eval(&parsed, ctx);
            }
            Ok(Value::Null)
        }
    }
}

fn eval_binop(op: &BinOp, left: &Expr, right: &Expr, ctx: &EvalContext) -> Result<Value> {
    // Short-circuit evaluation for && and ||
    match op {
        BinOp::And => {
            let lval = eval(left, ctx)?;
            if !lval.is_truthy() {
                return Ok(Value::Bool(false));
            }
            let rval = eval(right, ctx)?;
            return Ok(Value::Bool(rval.is_truthy()));
        }
        BinOp::Or => {
            let lval = eval(left, ctx)?;
            if lval.is_truthy() {
                return Ok(Value::Bool(true));
            }
            let rval = eval(right, ctx)?;
            return Ok(Value::Bool(rval.is_truthy()));
        }
        _ => {}
    }

    let lval = eval(left, ctx)?;
    let rval = eval(right, ctx)?;

    match op {
        BinOp::Add => eval_add(lval, rval),
        BinOp::Sub => eval_sub(lval, rval),
        BinOp::Mul => eval_arith(lval, rval, |a, b| a * b, "multiply"),
        BinOp::Div => eval_arith(lval, rval, |a, b| a / b, "divide"),
        BinOp::Mod => eval_arith(lval, rval, |a, b| a % b, "modulo"),
        BinOp::Eq => Ok(Value::Bool(values_equal(&lval, &rval))),
        BinOp::Ne => Ok(Value::Bool(!values_equal(&lval, &rval))),
        BinOp::Gt => Ok(Value::Bool(matches!(
            compare_values(&lval, &rval),
            std::cmp::Ordering::Greater
        ))),
        BinOp::Lt => Ok(Value::Bool(matches!(
            compare_values(&lval, &rval),
            std::cmp::Ordering::Less
        ))),
        BinOp::Ge => Ok(Value::Bool(!matches!(
            compare_values(&lval, &rval),
            std::cmp::Ordering::Less
        ))),
        BinOp::Le => Ok(Value::Bool(!matches!(
            compare_values(&lval, &rval),
            std::cmp::Ordering::Greater
        ))),
        BinOp::And | BinOp::Or => unreachable!("handled above"),
    }
}

fn eval_add(lval: Value, rval: Value) -> Result<Value> {
    match (&lval, &rval) {
        // Date + duration string (only when the string looks like a duration)
        (Value::Date(dt), Value::Str(s)) => {
            if let Some((amount, unit)) = parse_duration_string(s) {
                apply_duration(*dt, amount, unit).map(Value::Date)
            } else {
                Ok(Value::Str(format!("{}{}", lval.to_display(), s)))
            }
        }
        (Value::Number(a), Value::Number(b)) => Ok(Value::Number(a + b)),
        (Value::Str(a), Value::Str(b)) => Ok(Value::Str(format!("{a}{b}"))),
        (Value::Str(a), other) => Ok(Value::Str(format!("{}{}", a, other.to_display()))),
        (other, Value::Str(b)) => Ok(Value::Str(format!("{}{}", other.to_display(), b))),
        _ => {
            // Try numeric
            if let (Some(a), Some(b)) = (lval.as_number(), rval.as_number()) {
                Ok(Value::Number(a + b))
            } else {
                Ok(Value::Null)
            }
        }
    }
}

fn eval_sub(lval: Value, rval: Value) -> Result<Value> {
    match (&lval, &rval) {
        (Value::Date(a), Value::Date(b)) => Ok(Value::Number(
            a.signed_duration_since(*b).num_milliseconds() as f64,
        )),
        (Value::Date(dt), Value::Str(s)) => {
            let (amount, unit) = parse_duration_string(s)
                .ok_or_else(|| CrabaseError::ExprEval(format!("Cannot parse duration: {s:?}")))?;
            apply_duration(*dt, -amount, unit).map(Value::Date)
        }
        _ => eval_arith(lval, rval, |a, b| a - b, "subtract"),
    }
}

fn eval_arith(
    lval: Value,
    rval: Value,
    op: impl Fn(f64, f64) -> f64,
    op_name: &str,
) -> Result<Value> {
    // Null propagates through arithmetic (SQL/spreadsheet semantics)
    if matches!(lval, Value::Null) || matches!(rval, Value::Null) {
        return Ok(Value::Null);
    }
    match (lval.as_number(), rval.as_number()) {
        (Some(a), Some(b)) => Ok(Value::Number(op(a, b))),
        _ => Err(CrabaseError::ExprEval(format!(
            "Cannot {op_name} {lval:?} and {rval:?}"
        ))),
    }
}

pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Date(a), Value::Date(b)) => a == b,
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| values_equal(x, y))
        }
        // Cross-type numeric comparison
        (Value::Number(a), Value::Str(b)) => b.parse::<f64>() == Ok(*a),
        (Value::Str(a), Value::Number(b)) => a.parse::<f64>() == Ok(*b),
        _ => false,
    }
}

pub fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
        }
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Date(a), Value::Date(b)) => a.cmp(b),
        // Try numeric cross-type
        _ => {
            if let (Some(a), Some(b)) = (a.as_number(), b.as_number()) {
                a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                a.to_display().cmp(&b.to_display())
            }
        }
    }
}
