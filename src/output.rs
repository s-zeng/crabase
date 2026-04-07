use crate::base_file::BaseFile;
use crate::query::ResultRow;
use std::io::Write;

/// Convert a serde_yaml::Value to a CSV cell string
pub fn yaml_value_to_csv_str(val: &serde_yaml::Value) -> String {
    match val {
        serde_yaml::Value::Null => String::new(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(f) = n.as_f64() {
                // Trim unnecessary decimal places
                if f.fract() == 0.0 {
                    format!("{}", f as i64)
                } else {
                    format!("{f}")
                }
            } else {
                n.to_string()
            }
        }
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .map(yaml_value_to_csv_str)
            .collect::<Vec<_>>()
            .join(", "),
        serde_yaml::Value::Mapping(_) => String::new(),
        serde_yaml::Value::Tagged(tagged) => yaml_value_to_csv_str(&tagged.value),
    }
}

/// Write a single CSV field (quoting if necessary)
fn write_csv_field(out: &mut dyn Write, field: &str) -> std::io::Result<()> {
    let needs_quoting =
        field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r');
    if needs_quoting {
        // Escape quotes by doubling them
        let escaped = field.replace('"', "\"\"");
        write!(out, "\"{escaped}\"")
    } else {
        write!(out, "{field}")
    }
}

/// Get column header name (use displayName if configured)
fn get_header(col: &str, base_file: &BaseFile) -> String {
    if let Some(prop) = base_file.properties.get(col) {
        if let Some(display) = &prop.display_name {
            return display.clone();
        }
    }
    // Default: use the column name itself
    col.to_string()
}

/// Write CSV output to writer
pub fn write_csv(
    out: &mut dyn Write,
    columns: &[String],
    rows: &[ResultRow],
    base_file: &BaseFile,
) -> std::io::Result<()> {
    // Header row
    for (i, col) in columns.iter().enumerate() {
        if i > 0 {
            write!(out, ",")?;
        }
        let header = get_header(col, base_file);
        write_csv_field(out, &header)?;
    }
    writeln!(out)?;

    // Data rows
    for row in rows {
        for (i, val) in row.columns.iter().enumerate() {
            if i > 0 {
                write!(out, ",")?;
            }
            let cell = yaml_value_to_csv_str(val);
            write_csv_field(out, &cell)?;
        }
        writeln!(out)?;
    }

    Ok(())
}
