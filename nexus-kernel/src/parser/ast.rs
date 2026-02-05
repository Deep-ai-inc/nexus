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
    Function(FunctionDef),
    Case(CaseStatement),
    Watch(WatchStatement),
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

/// A function definition: name() { body }
#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub name: String,
    pub body: Vec<Command>,
}

/// A case statement: case word in pattern) commands ;; ... esac
#[derive(Debug, Clone)]
pub struct CaseStatement {
    pub word: Word,
    pub cases: Vec<CaseItem>,
}

/// A single case item: pattern) commands ;;
#[derive(Debug, Clone)]
pub struct CaseItem {
    pub patterns: Vec<String>,
    pub commands: Vec<Command>,
}

/// A watch statement: watch [-n interval] pipeline
#[derive(Debug, Clone)]
pub struct WatchStatement {
    pub interval_ms: u64,
    pub pipeline: Pipeline,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Word tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_word_as_literal_with_literal() {
        let word = Word::Literal("hello".to_string());
        assert_eq!(word.as_literal(), Some("hello"));
    }

    #[test]
    fn test_word_as_literal_with_variable() {
        let word = Word::Variable("HOME".to_string());
        assert_eq!(word.as_literal(), None);
    }

    #[test]
    fn test_word_as_literal_with_command_substitution() {
        let word = Word::CommandSubstitution("echo hello".to_string());
        assert_eq!(word.as_literal(), None);
    }

    // -------------------------------------------------------------------------
    // ListOperator tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_list_operator_equality() {
        assert_eq!(ListOperator::And, ListOperator::And);
        assert_eq!(ListOperator::Or, ListOperator::Or);
        assert_eq!(ListOperator::Semi, ListOperator::Semi);
        assert_eq!(ListOperator::Background, ListOperator::Background);
        assert_ne!(ListOperator::And, ListOperator::Or);
    }

    // -------------------------------------------------------------------------
    // RedirectOp tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_redirect_op_equality() {
        assert_eq!(RedirectOp::Write, RedirectOp::Write);
        assert_eq!(RedirectOp::Append, RedirectOp::Append);
        assert_eq!(RedirectOp::Read, RedirectOp::Read);
        assert_ne!(RedirectOp::Write, RedirectOp::Append);
    }

    // -------------------------------------------------------------------------
    // Debug trait tests (ensure Clone/Debug derive works)
    // -------------------------------------------------------------------------

    #[test]
    fn test_simple_command_debug() {
        let cmd = SimpleCommand {
            name: "ls".to_string(),
            args: vec![Word::Literal("-la".to_string())],
            redirects: vec![],
            env_assignments: vec![],
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("ls"));
    }

    #[test]
    fn test_pipeline_debug() {
        let pipeline = Pipeline {
            commands: vec![],
            background: false,
        };
        let debug_str = format!("{:?}", pipeline);
        assert!(debug_str.contains("background"));
    }

    #[test]
    fn test_redirect_debug() {
        let redirect = Redirect {
            fd: 1,
            op: RedirectOp::Write,
            target: "output.txt".to_string(),
        };
        let debug_str = format!("{:?}", redirect);
        assert!(debug_str.contains("output.txt"));
    }

    #[test]
    fn test_assignment_debug() {
        let assignment = Assignment {
            name: "PATH".to_string(),
            value: Word::Literal("/usr/bin".to_string()),
        };
        let debug_str = format!("{:?}", assignment);
        assert!(debug_str.contains("PATH"));
    }

    #[test]
    fn test_for_statement_debug() {
        let for_stmt = ForStatement {
            variable: "i".to_string(),
            items: vec![Word::Literal("1".to_string())],
            body: vec![],
        };
        let debug_str = format!("{:?}", for_stmt);
        assert!(debug_str.contains("variable"));
    }

    #[test]
    fn test_case_statement_debug() {
        let case_stmt = CaseStatement {
            word: Word::Variable("x".to_string()),
            cases: vec![CaseItem {
                patterns: vec!["*.txt".to_string()],
                commands: vec![],
            }],
        };
        let debug_str = format!("{:?}", case_stmt);
        assert!(debug_str.contains("patterns"));
    }

    #[test]
    fn test_watch_statement_debug() {
        let watch = WatchStatement {
            interval_ms: 2000,
            pipeline: Pipeline {
                commands: vec![],
                background: false,
            },
        };
        let debug_str = format!("{:?}", watch);
        assert!(debug_str.contains("interval_ms"));
    }

    // -------------------------------------------------------------------------
    // Clone trait tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_word_clone() {
        let word = Word::Literal("hello".to_string());
        let cloned = word.clone();
        assert_eq!(word.as_literal(), cloned.as_literal());
    }

    #[test]
    fn test_simple_command_clone() {
        let cmd = SimpleCommand {
            name: "echo".to_string(),
            args: vec![Word::Literal("hello".to_string())],
            redirects: vec![],
            env_assignments: vec![],
        };
        let cloned = cmd.clone();
        assert_eq!(cmd.name, cloned.name);
    }
}
