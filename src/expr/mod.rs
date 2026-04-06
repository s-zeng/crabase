pub mod ast;
pub mod eval;
pub mod lexer;
pub mod parser;

pub use eval::{EvalContext, eval};
pub use parser::parse;
