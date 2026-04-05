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
    // Method call: expr.method(args...)
    MethodCall {
        object: Box<Expr>,
        method: String,
        args: Vec<Expr>,
    },
    // Index access: expr[index]
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    // Function call: name(args...)
    FuncCall {
        name: String,
        args: Vec<Expr>,
    },
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
