/// AST nodes for the expression language

#[derive(Debug, Clone)]
pub enum Expr {
    // Literals
    Number(f64),
    Str(String),
    Bool(bool),
    Null,
    // Variable reference (bare identifier or dotted path)
    Ident(String),
    // Binary operations
    BinOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    // Unary operations
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    // Member access: expr.field
    Member {
        object: Box<Expr>,
        field: String,
    },
    // Index access: expr[index]
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    // Function or method call: callee(args...)
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    // Array literal: [e1, e2, ...]
    Array(Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}
