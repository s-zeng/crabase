//! Convert a `DataFrame` to CSV or TOON bytes. Both formats share the same
//! header transforms and primitive cell formatting; TOON additionally joins
//! list cells into strings so the encoder picks the compact tabular form.

use std::fmt::Write as _;
use std::io::Write;

use polars::prelude::*;
use serde_json::{Map, Number, Value};

use crate::base_file::BaseFile;

/// Header transformation: `displayName` override, strip `formula.` prefix,
/// alias well-known `file.*` columns to Obsidian's friendly names, then fall
/// back to replacing `.` with space.
fn header_for(col: &str, base_file: &BaseFile) -> String {
    if let Some(prop) = base_file.properties.get(col) {
        if let Some(display) = &prop.display_name {
            return display.clone();
        }
    }
    if let Some(name) = col.strip_prefix("formula.") {
        return name.to_string();
    }
    match col {
        "file.mtime" => return "modified time".to_string(),
        "file.ctime" => return "created time".to_string(),
        "file.folder" => return "folder".to_string(),
        "file.ext" => return "file extension".to_string(),
        "file.size" => return "file size".to_string(),
        "file.path" => return "file path".to_string(),
        "file.tags" => return "tags".to_string(),
        "file.links" => return "links".to_string(),
        _ => {}
    }
    col.replace('.', " ")
}

fn needs_csv_quoting(field: &str) -> bool {
    field
        .bytes()
        .any(|b| matches!(b, b',' | b'"' | b'\n' | b'\r'))
}

fn write_field(out: &mut dyn Write, field: &str) -> std::io::Result<()> {
    if needs_csv_quoting(field) {
        out.write_all(b"\"")?;
        // Stream the field in runs delimited by `"` so we double the quote
        // characters without allocating an intermediate `String`.
        let mut rest = field.as_bytes();
        while let Some(idx) = rest.iter().position(|&b| b == b'"') {
            out.write_all(&rest[..idx])?;
            out.write_all(b"\"\"")?;
            rest = &rest[idx + 1..];
        }
        out.write_all(rest)?;
        out.write_all(b"\"")
    } else {
        out.write_all(field.as_bytes())
    }
}

/// Write the DataFrame as CSV to `out`. Column order is taken from the DataFrame
/// (which `execute_query` arranges to match `view.order`).
pub fn write_csv(
    out: &mut dyn Write,
    columns: &[String],
    df: &DataFrame,
    base_file: &BaseFile,
) -> std::io::Result<()> {
    // Header row.
    for (i, col) in columns.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        write_field(out, &header_for(col, base_file))?;
    }
    out.write_all(b"\n")?;

    if columns.is_empty() {
        return Ok(());
    }

    let series = resolve_series(df, columns)?;
    let column_is_tags: Vec<bool> = columns.iter().map(is_tag_column).collect();

    let mut scratch = String::new();
    for row in 0..df.height() {
        for (i, s) in series.iter().enumerate() {
            if i > 0 {
                out.write_all(b",")?;
            }
            if let Ok(av) = s.get(row) {
                write_csv_cell(out, &av, column_is_tags[i], &mut scratch)?;
            }
        }
        out.write_all(b"\n")?;
    }
    Ok(())
}

/// Resolve a slice of column names to the underlying Series, failing fast when
/// any column is missing so callers don't silently emit truncated output.
fn resolve_series<'a>(df: &'a DataFrame, columns: &[String]) -> std::io::Result<Vec<&'a Series>> {
    columns
        .iter()
        .map(|name| {
            df.column(name)
                .map(|c| c.as_materialized_series())
                .map_err(|_| {
                    std::io::Error::other(format!("DataFrame missing requested column: {name:?}"))
                })
        })
        .collect()
}

/// True when this column should render list elements with a leading `#` —
/// i.e. it sources from the reserved `file.tags` list. Frontmatter `tags:` is
/// commonly aliased through to the same data, so a column whose underlying
/// dataframe series is named `file_tags` qualifies too.
fn is_tag_column(col: impl AsRef<str>) -> bool {
    matches!(col.as_ref(), "file.tags" | "tags")
}

/// Write a single CSV cell directly to `out`, going through the `scratch`
/// `String` buffer only for cell types that may contain a delimiter / quote.
fn write_csv_cell(
    out: &mut dyn Write,
    av: &AnyValue<'_>,
    tag_list: bool,
    scratch: &mut String,
) -> std::io::Result<()> {
    match av {
        AnyValue::Null => Ok(()),
        AnyValue::Boolean(b) => write!(out, "{b}"),
        AnyValue::Int8(n) => write!(out, "{n}"),
        AnyValue::Int16(n) => write!(out, "{n}"),
        AnyValue::Int32(n) => write!(out, "{n}"),
        AnyValue::Int64(n) => write!(out, "{n}"),
        AnyValue::UInt8(n) => write!(out, "{n}"),
        AnyValue::UInt16(n) => write!(out, "{n}"),
        AnyValue::UInt32(n) => write!(out, "{n}"),
        AnyValue::UInt64(n) => write!(out, "{n}"),
        AnyValue::Float32(f) => write_float(out, *f as f64),
        AnyValue::Float64(f) => write_float(out, *f),
        AnyValue::Duration(ms, _) => write!(out, "{ms}"),
        AnyValue::String(s) => write_field(out, s),
        AnyValue::StringOwned(s) => write_field(out, s),
        _ => {
            scratch.clear();
            format_into(scratch, av, tag_list);
            write_field(out, scratch)
        }
    }
}

fn write_float(out: &mut dyn Write, f: f64) -> std::io::Result<()> {
    if f.is_nan() {
        Ok(())
    } else if f.fract() == 0.0 && f.abs() < 1e15 {
        write!(out, "{}", f as i64)
    } else {
        write!(out, "{f}")
    }
}

/// Buffered formatter used by both the TOON path (where we need a `String`) and
/// the CSV fallback for date/datetime/list cells (which may contain commas).
fn format_into(out: &mut String, v: &AnyValue<'_>, tag_list: bool) {
    match v {
        AnyValue::Null => {}
        AnyValue::Boolean(b) => {
            let _ = write!(out, "{b}");
        }
        AnyValue::String(s) => out.push_str(s),
        AnyValue::StringOwned(s) => out.push_str(s),
        AnyValue::Int8(n) => {
            let _ = write!(out, "{n}");
        }
        AnyValue::Int16(n) => {
            let _ = write!(out, "{n}");
        }
        AnyValue::Int32(n) => {
            let _ = write!(out, "{n}");
        }
        AnyValue::Int64(n) => {
            let _ = write!(out, "{n}");
        }
        AnyValue::UInt8(n) => {
            let _ = write!(out, "{n}");
        }
        AnyValue::UInt16(n) => {
            let _ = write!(out, "{n}");
        }
        AnyValue::UInt32(n) => {
            let _ = write!(out, "{n}");
        }
        AnyValue::UInt64(n) => {
            let _ = write!(out, "{n}");
        }
        AnyValue::Float32(f) => format_float_into(out, *f as f64),
        AnyValue::Float64(f) => format_float_into(out, *f),
        AnyValue::Date(days) => {
            let base = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
            let d = base + chrono::Duration::days(*days as i64);
            let _ = write!(out, "{}", d.format("%Y-%m-%d"));
        }
        AnyValue::Datetime(micros, tu, _) | AnyValue::DatetimeOwned(micros, tu, _) => {
            let (secs, nsec) = micros_split(*micros, *tu);
            if let Some(dt) = chrono::DateTime::from_timestamp(secs, nsec) {
                let _ = write!(out, "{}", dt.naive_utc().format("%Y-%m-%dT%H:%M:%S"));
            }
        }
        AnyValue::Duration(ms, _) => {
            let _ = write!(out, "{ms}");
        }
        AnyValue::List(series) => append_csv_list(out, series, tag_list),
        other => {
            let _ = write!(out, "{other}");
        }
    }
}

fn micros_split(value: i64, tu: TimeUnit) -> (i64, u32) {
    match tu {
        TimeUnit::Nanoseconds => (value / 1_000_000_000, (value % 1_000_000_000) as u32),
        TimeUnit::Microseconds => (value / 1_000_000, ((value % 1_000_000) * 1_000) as u32),
        TimeUnit::Milliseconds => (value / 1_000, ((value % 1_000) * 1_000_000) as u32),
    }
}

/// Fold list elements into `out`, joining with `", "`. Tag columns prepend `#`
/// to non-empty elements that lack one. Skipping the intermediate Vec<String>
/// the old impl built keeps the hot row-format path allocation-light.
fn append_csv_list(out: &mut String, series: &Series, tag_list: bool) {
    let len = series.len();
    let start = out.len();
    for i in 0..len {
        if out.len() > start {
            out.push_str(", ");
        }
        let Ok(av) = series.get(i) else { continue };
        let elem_start = out.len();
        format_into(out, &av, false);
        if tag_list && out.len() > elem_start && !out[elem_start..].starts_with('#') {
            out.insert(elem_start, '#');
        }
    }
}

/// String-returning wrapper kept for the TOON path which needs a `Value::String`.
fn series_to_csv_list(series: &Series, tag_list: bool) -> String {
    let mut s = String::new();
    append_csv_list(&mut s, series, tag_list);
    s
}

fn format_float_into(out: &mut String, f: f64) {
    if f.is_nan() {
        return;
    }
    if f.fract() == 0.0 && f.abs() < 1e15 {
        let _ = write!(out, "{}", f as i64);
    } else {
        let _ = write!(out, "{f}");
    }
}

/// Write the DataFrame as TOON to `out`. Rows become an array of flat objects
/// keyed by the same column headers as `write_csv`; list-typed cells are
/// joined with `", "` so the encoder emits the compact tabular header
/// `[N]{col1,col2,...}:` rather than per-row key-value blocks.
pub fn write_toon(
    out: &mut dyn Write,
    columns: &[String],
    df: &DataFrame,
    base_file: &BaseFile,
) -> std::io::Result<()> {
    let series = resolve_series(df, columns)?;
    let headers: Vec<String> = columns.iter().map(|c| header_for(c, base_file)).collect();
    let column_is_tags: Vec<bool> = columns.iter().map(is_tag_column).collect();

    let rows: Vec<Value> = (0..df.height())
        .map(|row| {
            let obj: Map<String, Value> = series
                .iter()
                .zip(headers.iter())
                .zip(column_is_tags.iter())
                .map(|((s, header), &tag_list)| {
                    let json = match s.get(row) {
                        Ok(av) => any_value_to_json(&av, tag_list),
                        Err(_) => Value::Null,
                    };
                    (header.clone(), json)
                })
                .collect();
            Value::Object(obj)
        })
        .collect();

    let toon = toon_format::encode_default(&Value::Array(rows))
        .map_err(|e| std::io::Error::other(format!("toon encode failed: {e}")))?;
    out.write_all(toon.as_bytes())?;
    out.write_all(b"\n")?;
    Ok(())
}

fn any_value_to_json(v: &AnyValue<'_>, tag_list: bool) -> Value {
    match v {
        AnyValue::Null => Value::Null,
        AnyValue::Boolean(b) => Value::Bool(*b),
        AnyValue::String(s) => Value::String((*s).to_string()),
        AnyValue::StringOwned(s) => Value::String(s.to_string()),
        AnyValue::Int8(n) => Value::Number((*n as i64).into()),
        AnyValue::Int16(n) => Value::Number((*n as i64).into()),
        AnyValue::Int32(n) => Value::Number((*n as i64).into()),
        AnyValue::Int64(n) => Value::Number((*n).into()),
        AnyValue::UInt8(n) => Value::Number((*n as u64).into()),
        AnyValue::UInt16(n) => Value::Number((*n as u64).into()),
        AnyValue::UInt32(n) => Value::Number((*n as u64).into()),
        AnyValue::UInt64(n) => Value::Number((*n).into()),
        AnyValue::Float32(f) => float_to_json(*f as f64),
        AnyValue::Float64(f) => float_to_json(*f),
        AnyValue::Date(_) | AnyValue::Datetime(_, _, _) | AnyValue::DatetimeOwned(_, _, _) => {
            let mut s = String::new();
            format_into(&mut s, v, false);
            Value::String(s)
        }
        AnyValue::Duration(ms, _) => Value::Number((*ms).into()),
        AnyValue::List(series) => Value::String(series_to_csv_list(series, tag_list)),
        other => Value::String(format!("{other}")),
    }
}

fn float_to_json(f: f64) -> Value {
    if f.is_nan() {
        return Value::Null;
    }
    if f.fract() == 0.0 && f.abs() < 1e15 {
        Value::Number((f as i64).into())
    } else {
        Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}
