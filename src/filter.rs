use crate::base_file::FilterNode;
use crate::error::Result;
use crate::expr::{eval, parse, EvalContext};
use crate::vault::VaultFile;
use std::collections::HashMap;

/// Evaluate a FilterNode against a VaultFile, returning true if the file passes
pub fn eval_filter(
    node: &FilterNode,
    file: &VaultFile,
    formulas: &HashMap<String, String>,
) -> Result<bool> {
    let ctx = EvalContext::new(file.file_props(), file.note_props(), formulas.clone());
    eval_filter_with_ctx(node, &ctx)
}

fn eval_filter_with_ctx(node: &FilterNode, ctx: &EvalContext) -> Result<bool> {
    match node {
        FilterNode::And(children) => {
            for child in children {
                if !eval_filter_with_ctx(child, ctx)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        FilterNode::Or(children) => {
            for child in children {
                if eval_filter_with_ctx(child, ctx)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        FilterNode::Not(children) => {
            // "Not" means none of the children are true (i.e., NOT OR)
            for child in children {
                if eval_filter_with_ctx(child, ctx)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        FilterNode::Expr(expr_str) => {
            let ast = parse(expr_str)?;
            let val = eval(&ast, ctx)?;
            Ok(val.is_truthy())
        }
    }
}

/// Apply an optional global filter and view filter to a file
/// Returns true if the file should be included in results
pub fn file_passes_filters(
    file: &VaultFile,
    global_filter: Option<&FilterNode>,
    view_filter: Option<&FilterNode>,
    formulas: &HashMap<String, String>,
) -> Result<bool> {
    let ctx = EvalContext::new(file.file_props(), file.note_props(), formulas.clone());

    if let Some(global) = global_filter {
        if !eval_filter_with_ctx(global, &ctx)? {
            return Ok(false);
        }
    }

    if let Some(view) = view_filter {
        if !eval_filter_with_ctx(view, &ctx)? {
            return Ok(false);
        }
    }

    Ok(true)
}
