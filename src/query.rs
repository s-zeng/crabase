use std::collections::HashMap;
use std::path::Path;

use crate::base_file::{BaseFile, SortDirection, View};
use crate::error::Result;
use crate::expr::eval::compare_values;
use crate::expr::{EvalContext, eval, parse};
use crate::filter::file_passes_filters;
use crate::vault::{VaultFile, scan_vault};

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
    let matched = files
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
                Err(error) => Some(Err(error)),
            }
        })
        .collect::<Result<Vec<_>>>()?;
    let columns = view.order.as_deref().unwrap_or(&[]);

    sort_files(matched, view)
        .into_iter()
        .take(view.limit.unwrap_or(usize::MAX))
        .map(|file| {
            columns
                .iter()
                .map(|column| extract_column(&file, column, &base_file.formulas))
                .collect::<Result<Vec<_>>>()
                .map(|columns| ResultRow { columns })
        })
        .collect()
}

fn sort_keys(view: &View) -> Vec<(&str, &SortDirection)> {
    view.group_by
        .iter()
        .map(|group_by| (group_by.property.as_str(), &group_by.direction))
        .chain(view.sort.iter().flat_map(|keys| {
            keys.iter()
                .map(|key| (key.property.as_str(), &key.direction))
        }))
        .collect()
}

/// Sort files according to view's groupBy and sort fields
fn sort_files(
    files: Vec<VaultFile>,
    view: &View,
) -> Vec<VaultFile> {
    let sort_keys = sort_keys(view);
    if sort_keys.is_empty() {
        return files;
    }

    let mut sorted = files;
    sorted.sort_by(|left, right| {
        sort_keys
            .iter()
            .map(|(property, direction)| {
                let ord = compare_values(
                    &get_sort_value(left, property),
                    &get_sort_value(right, property),
                );
                match direction {
                    SortDirection::Asc => ord,
                    SortDirection::Desc => ord.reverse(),
                }
            })
            .find(|ord| *ord != std::cmp::Ordering::Equal)
            .unwrap_or_else(|| left.stem.cmp(&right.stem))
    });
    sorted
}

fn get_sort_value(file: &VaultFile, prop: &str) -> crate::expr::eval::Value {
    use crate::expr::eval::Value;

    match prop.split_once('.') {
        Some(("file", field)) => file.file_props().get(field).cloned().unwrap_or(Value::Null),
        Some(("note", field)) => file.note_props().get(field).cloned().unwrap_or(Value::Null),
        _ => file.note_props().get(prop).cloned().unwrap_or(Value::Null),
    }
}

/// Extract a column value for a file
fn extract_column(
    file: &VaultFile,
    column: &str,
    formulas: &HashMap<String, String>,
) -> Result<serde_yaml::Value> {
    if column == "title" {
        let display = file
            .frontmatter
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(&file.stem)
            .to_string();
        return Ok(serde_yaml::Value::String(format!(
            "[[{}| {}]]",
            file.rel_path, display
        )));
    }

    if let Some(field) = column.strip_prefix("file.") {
        let value = file
            .file_props()
            .get(field)
            .cloned()
            .unwrap_or(crate::expr::eval::Value::Null);
        return Ok(value_to_yaml(&value));
    }

    if let Some(formula_name) = column.strip_prefix("formula.") {
        return formulas
            .get(formula_name)
            .map(|expr_str| {
                let ctx = EvalContext::new(file.file_props(), file.note_props(), formulas.clone());
                let ast = parse(expr_str)?;
                let value = eval(&ast, &ctx)?;
                Ok(value_to_yaml(&value))
            })
            .transpose()
            .map(|value| value.unwrap_or(serde_yaml::Value::Null));
    }

    if let Some(field) = column.strip_prefix("note.") {
        return Ok(file
            .frontmatter
            .get(field)
            .cloned()
            .unwrap_or(serde_yaml::Value::Null));
    }

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
        Value::Number(n) => serde_yaml::to_value(*n).unwrap_or(serde_yaml::Value::Null),
        Value::Str(s) => serde_yaml::Value::String(s.clone()),
        Value::Date(dt) => serde_yaml::Value::String(dt.format("%Y-%m-%d %H:%M:%S").to_string()),
        Value::List(items) => {
            serde_yaml::Value::Sequence(items.iter().map(value_to_yaml).collect())
        }
    }
}
