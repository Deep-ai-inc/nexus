//! Shell error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ShellError {
    #[error("parse error: {0}")]
    Parse(String),

    #[error("command not found: {0}")]
    CommandNotFound(String),

    #[error("syntax error: {0}")]
    Syntax(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("nix error: {0}")]
    Nix(#[from] nix::Error),

    #[error("{0}")]
    Other(String),
}
