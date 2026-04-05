pub mod ast;
pub mod eval;
pub mod lexer;
pub mod parser;

pub use eval::{eval, EvalContext};
pub use parser::parse;
