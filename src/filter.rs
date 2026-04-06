use crate::base_file::FilterNode;
use crate::error::Result;
use crate::expr::{EvalContext, eval, parse};
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
        FilterNode::And(children) => children.iter().try_fold(true, |acc, child| {
            if !acc {
                Ok(false)
            } else {
                eval_filter_with_ctx(child, ctx)
            }
        }),
        FilterNode::Or(children) => children.iter().try_fold(false, |acc, child| {
            if acc {
                Ok(true)
            } else {
                eval_filter_with_ctx(child, ctx)
            }
        }),
        FilterNode::Not(children) => children.iter().try_fold(true, |acc, child| {
            if !acc {
                Ok(false)
            } else {
                eval_filter_with_ctx(child, ctx).map(|matches| !matches)
            }
        }),
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
    [global_filter, view_filter]
        .into_iter()
        .flatten()
        .try_fold(true, |acc, filter| {
            if !acc {
                Ok(false)
            } else {
                eval_filter_with_ctx(filter, &ctx)
            }
        })
}
