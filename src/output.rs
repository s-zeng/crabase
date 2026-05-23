//! Convert a `DataFrame` to CSV bytes, applying the same header transforms and
//! cell formatting as the legacy writer (so snapshots largely survive).

use std::io::Write;

use polars::prelude::*;

use crate::base_file::BaseFile;

/// Header transformation: `displayName` override, strip `formula.` prefix,
/// replace `.` with space (so `file.name` becomes `file name`).
fn header_for(col: &str, base_file: &BaseFile) -> String {
    if let Some(prop) = base_file.properties.get(col) {
        if let Some(display) = &prop.display_name {
            return display.clone();
        }
    }
    if let Some(name) = col.strip_prefix("formula.") {
        return name.to_string();
    }
    col.replace('.', " ")
}

fn write_field(out: &mut dyn Write, field: &str) -> std::io::Result<()> {
    let needs_quoting =
        field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r');
    if needs_quoting {
        let escaped = field.replace('"', "\"\"");
        write!(out, "\"{escaped}\"")
    } else {
        write!(out, "{field}")
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
    // Header row
    for (i, col) in columns.iter().enumerate() {
        if i > 0 {
            write!(out, ",")?;
        }
        let header = header_for(col, base_file);
        write_field(out, &header)?;
    }
    writeln!(out)?;

    if columns.is_empty() {
        return Ok(());
    }

    // Resolve series in the requested column order, falling back to an empty
    // series of length df.height() when a column is missing.
    let series: Vec<&Series> = columns
        .iter()
        .map(|name| df.column(name).ok().map(|c| c.as_materialized_series()))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default();

    if series.len() != columns.len() {
        // A column is missing — bail with an IO-ish error so callers see the
        // problem rather than silently emitting truncated output.
        return Err(std::io::Error::other(format!(
            "DataFrame missing one or more requested columns: {columns:?}"
        )));
    }

    for row in 0..df.height() {
        for (i, s) in series.iter().enumerate() {
            if i > 0 {
                write!(out, ",")?;
            }
            let cell = format_cell(s, row);
            write_field(out, &cell)?;
        }
        writeln!(out)?;
    }
    Ok(())
}

fn format_cell(s: &Series, row: usize) -> String {
    let Ok(v) = s.get(row) else {
        return String::new();
    };
    format_any(&v)
}

fn format_any(v: &AnyValue<'_>) -> String {
    match v {
        AnyValue::Null => String::new(),
        AnyValue::Boolean(b) => b.to_string(),
        AnyValue::String(s) => s.to_string(),
        AnyValue::StringOwned(s) => s.to_string(),
        AnyValue::Int8(n) => n.to_string(),
        AnyValue::Int16(n) => n.to_string(),
        AnyValue::Int32(n) => n.to_string(),
        AnyValue::Int64(n) => n.to_string(),
        AnyValue::UInt8(n) => n.to_string(),
        AnyValue::UInt16(n) => n.to_string(),
        AnyValue::UInt32(n) => n.to_string(),
        AnyValue::UInt64(n) => n.to_string(),
        AnyValue::Float32(f) => format_float(*f as f64),
        AnyValue::Float64(f) => format_float(*f),
        AnyValue::Date(days) => {
            // days since 1970-01-01
            let base = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
            (base + chrono::Duration::days(*days as i64))
                .format("%Y-%m-%d")
                .to_string()
        }
        AnyValue::Datetime(micros, tu, _) => {
            let secs = match tu {
                TimeUnit::Nanoseconds => micros / 1_000_000_000,
                TimeUnit::Microseconds => micros / 1_000_000,
                TimeUnit::Milliseconds => micros / 1_000,
            };
            let nsec = match tu {
                TimeUnit::Nanoseconds => (micros % 1_000_000_000) as u32,
                TimeUnit::Microseconds => ((micros % 1_000_000) * 1_000) as u32,
                TimeUnit::Milliseconds => ((micros % 1_000) * 1_000_000) as u32,
            };
            match chrono::DateTime::from_timestamp(secs, nsec) {
                Some(dt) => dt.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string(),
                None => String::new(),
            }
        }
        AnyValue::DatetimeOwned(micros, tu, _) => {
            format_any(&AnyValue::Datetime(*micros, *tu, None))
        }
        AnyValue::Duration(ms, _) => ms.to_string(),
        AnyValue::List(series) => series_to_csv_list(series),
        other => format!("{other}"),
    }
}

fn series_to_csv_list(series: &Series) -> String {
    let parts: Vec<String> = (0..series.len())
        .map(|i| series.get(i).map(|av| format_any(&av)).unwrap_or_default())
        .collect();
    parts.join(", ")
}

fn format_float(f: f64) -> String {
    if f.is_nan() {
        return String::new();
    }
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}
