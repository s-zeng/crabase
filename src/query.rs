use std::collections::HashMap;
use std::path::Path;

use crate::base_file::{BaseFile, SortDirection, View};
use crate::error::Result;
use crate::expr::{eval, parse, EvalContext};
use crate::expr::eval::compare_values;
use crate::filter::file_passes_filters;
use crate::vault::{scan_vault, VaultFile};

/// A single row of query results
#[derive(Debug)]
pub struct ResultRow {
    pub columns: Vec<serde_yaml::Value>,
}

/// Execute the query for a given view and return rows
pub fn execute_query(
    vault_root: &Path,
    base_file: &BaseFile,
    view: &View,
) -> Result<Vec<ResultRow>> {
    let files = scan_vault(vault_root)?;

    // Filter files
    let mut matched: Vec<VaultFile> = files
        .into_iter()
        .filter_map(|file| {
            match file_passes_filters(
                &file,
                base_file.filters.as_ref(),
                view.filters.as_ref(),
                &base_file.formulas,
            ) {
                Ok(true) => Some(Ok(file)),
                Ok(false) => None,
                Err(e) => Some(Err(e)),
            }
        })
        .collect::<Result<Vec<_>>>()?;

    // Sort: primary sort by groupBy, then additional sort keys
    sort_files(&mut matched, view);

    // Apply limit
    if let Some(limit) = view.limit {
        matched.truncate(limit);
    }

    // Extract columns
    let columns = view.order.as_deref().unwrap_or(&[]);
    let rows = matched
        .iter()
        .map(|file| {
            let cols = columns
                .iter()
                .map(|col| extract_column(file, col, &base_file.formulas))
                .collect::<Result<Vec<_>>>()?;
            Ok(ResultRow { columns: cols })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(rows)
}

/// Sort files according to view's groupBy and sort fields
fn sort_files(files: &mut Vec<VaultFile>, view: &View) {
    // Build sort keys: groupBy first, then sort array
    let mut sort_keys: Vec<(String, SortDirection)> = Vec::new();

    if let Some(group_by) = &view.group_by {
        sort_keys.push((group_by.property.clone(), group_by.direction.clone()));
    }

    if let Some(sort) = &view.sort {
        for key in sort {
            sort_keys.push((key.property.clone(), key.direction.clone()));
        }
    }

    if sort_keys.is_empty() {
        return;
    }

    files.sort_by(|a, b| {
        for (prop, direction) in &sort_keys {
            let a_val = get_sort_value(a, prop);
            let b_val = get_sort_value(b, prop);
            let ord = compare_values(&a_val, &b_val);
            let ord = match direction {
                SortDirection::Asc => ord,
                SortDirection::Desc => ord.reverse(),
            };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
}

fn get_sort_value(file: &VaultFile, prop: &str) -> crate::expr::eval::Value {
    use crate::expr::eval::Value;

    // Handle file properties
    if prop.starts_with("file.") {
        let field = &prop[5..];
        return file.file_props().get(field).cloned().unwrap_or(Value::Null);
    }

    // Handle note properties
    if prop.starts_with("note.") {
        let field = &prop[5..];
        return file.note_props().get(field).cloned().unwrap_or(Value::Null);
    }

    // Bare identifier = note property
    file.note_props().get(prop).cloned().unwrap_or(Value::Null)
}

/// Extract a column value for a file
fn extract_column(
    file: &VaultFile,
    column: &str,
    formulas: &HashMap<String, String>,
) -> Result<serde_yaml::Value> {
    // Special "title" column
    if column == "title" {
        let display = file
            .frontmatter
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(&file.stem)
            .to_string();
        let wikilink = format!("[[{}| {}]]", file.rel_path, display);
        return Ok(serde_yaml::Value::String(wikilink));
    }

    // file.* properties
    if column.starts_with("file.") {
        let field = &column[5..];
        let file_props = file.file_props();
        let val = file_props.get(field).cloned().unwrap_or(crate::expr::eval::Value::Null);
        return Ok(value_to_yaml(&val));
    }

    // formula.* properties
    if column.starts_with("formula.") {
        let formula_name = &column[8..];
        if let Some(expr_str) = formulas.get(formula_name) {
            let ctx = EvalContext::new(file.file_props(), file.note_props(), formulas.clone());
            let ast = parse(expr_str)?;
            let val = eval(&ast, &ctx)?;
            return Ok(value_to_yaml(&val));
        }
        return Ok(serde_yaml::Value::Null);
    }

    // note.* properties
    if column.starts_with("note.") {
        let field = &column[5..];
        return Ok(file
            .frontmatter
            .get(field)
            .cloned()
            .unwrap_or(serde_yaml::Value::Null));
    }

    // Bare identifier = note property
    Ok(file
        .frontmatter
        .get(column)
        .cloned()
        .unwrap_or(serde_yaml::Value::Null))
}

fn value_to_yaml(val: &crate::expr::eval::Value) -> serde_yaml::Value {
    use crate::expr::eval::Value;
    match val {
        Value::Null => serde_yaml::Value::Null,
        Value::Bool(b) => serde_yaml::Value::Bool(*b),
        Value::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                serde_yaml::Value::Number(serde_yaml::Number::from(*n as i64))
            } else {
                serde_yaml::Value::Number(
                    serde_yaml::Number::from(serde_yaml::Number::from(*n as i64)),
                )
            }
        }
        Value::Str(s) => serde_yaml::Value::String(s.clone()),
        Value::List(items) => {
            serde_yaml::Value::Sequence(items.iter().map(value_to_yaml).collect())
        }
    }
}
