use thiserror::Error;

#[derive(Debug, Error)]
pub enum CrabaseError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Base file error: {0}")]
    BaseFile(String),

    #[error("Expression parse error: {0}")]
    ExprParse(String),

    #[error("Expression eval error: {0}")]
    ExprEval(String),

    #[error("Filter error: {0}")]
    Filter(String),

    #[error("Query error: {0}")]
    Query(String),

    #[error("No views found in base file")]
    NoViews,

    #[error("View not found: {0}")]
    ViewNotFound(String),

    #[error("Missing required argument: {0}")]
    MissingArg(String),
}

pub type Result<T> = std::result::Result<T, CrabaseError>;
