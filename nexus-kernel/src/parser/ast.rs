//! Abstract Syntax Tree definitions.

/// The root AST node containing all commands.
#[derive(Debug, Clone)]
pub struct Ast {
    pub commands: Vec<Command>,
}

/// A shell command.
#[derive(Debug, Clone)]
pub enum Command {
    Simple(SimpleCommand),
    Pipeline(Pipeline),
    List(List),
    Subshell(Subshell),
    Assignment(Assignment),
    If(IfStatement),
    While(WhileStatement),
    For(ForStatement),
}

/// A simple command: name, arguments, redirections.
#[derive(Debug, Clone)]
pub struct SimpleCommand {
    pub name: String,
    pub args: Vec<Word>,
    pub redirects: Vec<Redirect>,
    pub env_assignments: Vec<Assignment>,
}

/// A word in the shell (may need expansion).
#[derive(Debug, Clone)]
pub enum Word {
    Literal(String),
    Variable(String),
    CommandSubstitution(String),
    // TODO: Glob patterns, brace expansion, etc.
}

impl Word {
    /// Get the literal value if this is a literal word.
    pub fn as_literal(&self) -> Option<&str> {
        match self {
            Word::Literal(s) => Some(s),
            _ => None,
        }
    }
}

/// A pipeline: cmd1 | cmd2 | cmd3
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub commands: Vec<Command>,
    pub background: bool,
}

/// A list: cmd1 && cmd2 || cmd3 ; cmd4
#[derive(Debug, Clone)]
pub struct List {
    pub items: Vec<Command>,
    pub operators: Vec<ListOperator>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListOperator {
    And,        // &&
    Or,         // ||
    Semi,       // ;
    Background, // &
}

/// A subshell: ( commands )
#[derive(Debug, Clone)]
pub struct Subshell {
    pub commands: Vec<Command>,
}

/// A variable assignment: NAME=value
#[derive(Debug, Clone)]
pub struct Assignment {
    pub name: String,
    pub value: Word,
}

/// A redirection: [n]op target
#[derive(Debug, Clone)]
pub struct Redirect {
    pub fd: i32,
    pub op: RedirectOp,
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectOp {
    Write,    // >
    Append,   // >>
    Read,     // <
    DupWrite, // >&
    DupRead,  // <&
}

/// An if statement.
#[derive(Debug, Clone)]
pub struct IfStatement {
    pub condition: Vec<Command>,
    pub then_branch: Vec<Command>,
    pub else_branch: Option<Vec<Command>>,
}

/// A while/until loop.
#[derive(Debug, Clone)]
pub struct WhileStatement {
    pub condition: Vec<Command>,
    pub body: Vec<Command>,
}

/// A for loop.
#[derive(Debug, Clone)]
pub struct ForStatement {
    pub variable: String,
    pub items: Vec<Word>,
    pub body: Vec<Command>,
}
