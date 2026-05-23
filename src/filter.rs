//! Compile a `FilterNode` tree into a single polars `Expr` predicate.
//!
//! `And` folds via `Expr::and`, `Or` via `Expr::or`, `Not` is the negation of
//! `Or` (matches the old per-row semantics: "none of the children are true").
//! Leaf `Expr` filters are parsed once and translated to a typed boolean
//! polars expression with `null` coerced to `false` to match the previous
//! `is_truthy()` semantics.

use polars::prelude::*;

use crate::base_file::FilterNode;
use crate::error::Result;
use crate::expr::{TranslateCtx, parse, translate, truthy};

/// Compile a `FilterNode` to a polars predicate expression.
pub fn filter_node_to_expr(node: &FilterNode, ctx: &TranslateCtx) -> Result<Expr> {
    let raw = compile(node, ctx)?;
    Ok(raw.fill_null(lit(false)))
}

fn compile(node: &FilterNode, ctx: &TranslateCtx) -> Result<Expr> {
    match node {
        FilterNode::And(children) => {
            if children.is_empty() {
                return Ok(lit(true));
            }
            let mut iter = children.iter();
            let first = compile(iter.next().unwrap(), ctx)?;
            iter.try_fold(first, |acc, child| Ok(acc.and(compile(child, ctx)?)))
        }
        FilterNode::Or(children) => {
            if children.is_empty() {
                return Ok(lit(false));
            }
            let mut iter = children.iter();
            let first = compile(iter.next().unwrap(), ctx)?;
            iter.try_fold(first, |acc, child| Ok(acc.or(compile(child, ctx)?)))
        }
        FilterNode::Not(children) => {
            // "none of these are true" → !any
            let any = compile(&FilterNode::Or(children.clone()), ctx)?;
            Ok(any.not())
        }
        FilterNode::Expr(expr_str) => {
            let ast = parse(expr_str)?;
            let translated = translate(&ast, ctx)?;
            Ok(truthy(translated))
        }
    }
}

/// Combine global and view filters into a single predicate. Returns `lit(true)`
/// if neither is present.
pub fn combine_filters(
    global: Option<&FilterNode>,
    view: Option<&FilterNode>,
    ctx: &TranslateCtx,
) -> Result<Expr> {
    match (global, view) {
        (None, None) => Ok(lit(true)),
        (Some(g), None) => filter_node_to_expr(g, ctx),
        (None, Some(v)) => filter_node_to_expr(v, ctx),
        (Some(g), Some(v)) => {
            let ge = filter_node_to_expr(g, ctx)?;
            let ve = filter_node_to_expr(v, ctx)?;
            Ok(ge.and(ve))
        }
    }
}
