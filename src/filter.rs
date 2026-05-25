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
        FilterNode::And(children) => fold_children(children, ctx, lit(true), Expr::and),
        FilterNode::Or(children) => fold_children(children, ctx, lit(false), Expr::or),
        FilterNode::Not(children) => {
            // "none of these are true" → !any. Borrow children directly instead
            // of cloning them just to wrap in another FilterNode::Or.
            Ok(fold_children(children, ctx, lit(false), Expr::or)?.not())
        }
        FilterNode::Expr(expr_str) => {
            let ast = parse(expr_str)?;
            let translated = translate(&ast, ctx)?;
            Ok(truthy(translated))
        }
    }
}

/// Reduce a child list into a single Expr via `combine`, using `empty` when the
/// list has no children. Shared between And/Or/Not so each branch stays a
/// one-liner.
fn fold_children(
    children: &[FilterNode],
    ctx: &TranslateCtx,
    empty: Expr,
    combine: fn(Expr, Expr) -> Expr,
) -> Result<Expr> {
    children
        .iter()
        .try_fold(None::<Expr>, |acc, child| {
            let next = compile(child, ctx)?;
            Ok(Some(match acc {
                Some(a) => combine(a, next),
                None => next,
            }))
        })
        .map(|opt| opt.unwrap_or(empty))
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
