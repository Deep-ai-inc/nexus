//! Parser - Tree-sitter integration and AST construction.

mod ast;

pub use ast::*;

use tree_sitter::Node;

use crate::ShellError;

/// The shell parser, wrapping Tree-sitter.
pub struct Parser {
    parser: tree_sitter::Parser,
}

impl Parser {
    /// Create a new parser.
    pub fn new() -> anyhow::Result<Self> {
        let mut parser = tree_sitter::Parser::new();
        let language = tree_sitter_bash::LANGUAGE;
        parser.set_language(&language.into())?;
        Ok(Self { parser })
    }

    /// Parse a command line into an AST.
    pub fn parse(&mut self, input: &str) -> Result<Ast, ShellError> {
        let tree = self
            .parser
            .parse(input, None)
            .ok_or_else(|| ShellError::Parse("failed to parse input".into()))?;

        let root = tree.root_node();
        if root.has_error() {
            // Find the error node for better diagnostics
            let error_msg = find_error_message(&root, input);
            return Err(ShellError::Syntax(error_msg));
        }

        build_ast(&root, input)
    }
}

/// Find the first error in the tree and return a descriptive message.
fn find_error_message(node: &Node, source: &str) -> String {
    if node.is_error() || node.is_missing() {
        let start = node.start_position();
        return format!(
            "unexpected token at line {}, column {}",
            start.row + 1,
            start.column + 1
        );
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let msg = find_error_message(&child, source);
        if !msg.is_empty() {
            return msg;
        }
    }

    String::new()
}

/// Build our AST from the Tree-sitter CST.
fn build_ast(node: &Node, source: &str) -> Result<Ast, ShellError> {
    let mut commands = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if let Some(cmd) = build_command(&child, source)? {
            commands.push(cmd);
        }
    }

    Ok(Ast { commands })
}

/// Build a command from a Tree-sitter node.
fn build_command(node: &Node, source: &str) -> Result<Option<Command>, ShellError> {
    match node.kind() {
        "command" => {
            let cmd = build_simple_command(node, source)?;
            Ok(Some(Command::Simple(cmd)))
        }
        "pipeline" => {
            let pipeline = build_pipeline(node, source)?;
            Ok(Some(Command::Pipeline(pipeline)))
        }
        "list" => {
            let list = build_list(node, source)?;
            Ok(Some(Command::List(list)))
        }
        "subshell" => {
            let subshell = build_subshell(node, source)?;
            Ok(Some(Command::Subshell(subshell)))
        }
        "redirected_statement" => {
            // Handle redirections wrapped around commands
            build_redirected_statement(node, source)
        }
        "variable_assignment" => {
            let assignment = build_assignment(node, source)?;
            Ok(Some(Command::Assignment(assignment)))
        }
        "if_statement" => {
            let if_stmt = build_if_statement(node, source)?;
            Ok(Some(Command::If(if_stmt)))
        }
        "while_statement" => {
            let while_stmt = build_while_statement(node, source)?;
            Ok(Some(Command::While(while_stmt)))
        }
        "for_statement" => {
            let for_stmt = build_for_statement(node, source)?;
            Ok(Some(Command::For(for_stmt)))
        }
        "comment" | "\n" => Ok(None),
        _ => {
            // Unknown node type - log and skip
            tracing::debug!("skipping unknown node type: {}", node.kind());
            Ok(None)
        }
    }
}

fn build_simple_command(node: &Node, source: &str) -> Result<SimpleCommand, ShellError> {
    let mut name = None;
    let mut args = Vec::new();
    let mut redirects = Vec::new();
    let mut env_assignments = Vec::new();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "command_name" => {
                name = Some(node_text(&child, source));
            }
            "word" | "string" | "raw_string" | "concatenation" | "number" => {
                if name.is_none() {
                    name = Some(node_text(&child, source));
                } else {
                    args.push(Word::Literal(node_text(&child, source)));
                }
            }
            "simple_expansion" | "expansion" => {
                args.push(Word::Variable(extract_variable_name(&child, source)));
            }
            "command_substitution" => {
                args.push(Word::CommandSubstitution(node_text(&child, source)));
            }
            "file_redirect" | "heredoc_redirect" => {
                if let Some(redir) = build_redirect(&child, source)? {
                    redirects.push(redir);
                }
            }
            "variable_assignment" => {
                let assignment = build_assignment(&child, source)?;
                env_assignments.push(assignment);
            }
            _ => {
                // Log unknown node types for debugging
                tracing::debug!(
                    "unknown node in simple_command: kind={}, text={}",
                    child.kind(),
                    node_text(&child, source)
                );
            }
        }
    }

    Ok(SimpleCommand {
        name: name.unwrap_or_default(),
        args,
        redirects,
        env_assignments,
    })
}

fn build_pipeline(node: &Node, source: &str) -> Result<Pipeline, ShellError> {
    let mut commands = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if let Some(cmd) = build_command(&child, source)? {
            commands.push(cmd);
        }
    }

    Ok(Pipeline {
        commands,
        background: false,
    })
}

fn build_list(node: &Node, source: &str) -> Result<List, ShellError> {
    let mut items = Vec::new();
    let mut operators = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "&&" => operators.push(ListOperator::And),
            "||" => operators.push(ListOperator::Or),
            ";" => operators.push(ListOperator::Semi),
            "&" => operators.push(ListOperator::Background),
            _ => {
                if let Some(cmd) = build_command(&child, source)? {
                    items.push(cmd);
                }
            }
        }
    }

    Ok(List { items, operators })
}

fn build_subshell(node: &Node, source: &str) -> Result<Subshell, ShellError> {
    let mut commands = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if let Some(cmd) = build_command(&child, source)? {
            commands.push(cmd);
        }
    }

    Ok(Subshell { commands })
}

fn build_redirected_statement(node: &Node, source: &str) -> Result<Option<Command>, ShellError> {
    let mut inner_cmd = None;
    let mut redirects = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "file_redirect" | "heredoc_redirect" => {
                if let Some(redir) = build_redirect(&child, source)? {
                    redirects.push(redir);
                }
            }
            _ => {
                if let Some(cmd) = build_command(&child, source)? {
                    inner_cmd = Some(cmd);
                }
            }
        }
    }

    // Apply redirects to the inner command
    if let Some(mut cmd) = inner_cmd {
        match &mut cmd {
            Command::Simple(simple) => {
                simple.redirects.extend(redirects);
            }
            _ => {
                // For non-simple commands, wrap in a simple command structure
                // This is a simplification; proper handling would need more work
            }
        }
        Ok(Some(cmd))
    } else {
        Ok(None)
    }
}

fn build_redirect(node: &Node, source: &str) -> Result<Option<Redirect>, ShellError> {
    let mut fd = None;
    let mut op = None;
    let mut target = None;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "file_descriptor" => {
                fd = node_text(&child, source).parse().ok();
            }
            ">" => op = Some(RedirectOp::Write),
            ">>" => op = Some(RedirectOp::Append),
            "<" => op = Some(RedirectOp::Read),
            ">&" => op = Some(RedirectOp::DupWrite),
            "<&" => op = Some(RedirectOp::DupRead),
            "word" | "string" => {
                target = Some(node_text(&child, source));
            }
            _ => {}
        }
    }

    match (op, target) {
        (Some(op), Some(target)) => Ok(Some(Redirect {
            fd: fd.unwrap_or(if matches!(op, RedirectOp::Read) { 0 } else { 1 }),
            op,
            target,
        })),
        _ => Ok(None),
    }
}

fn build_assignment(node: &Node, source: &str) -> Result<Assignment, ShellError> {
    let text = node_text(node, source);
    if let Some((name, value)) = text.split_once('=') {
        Ok(Assignment {
            name: name.to_string(),
            value: Word::Literal(value.to_string()),
        })
    } else {
        Err(ShellError::Syntax(format!(
            "invalid assignment: {}",
            text
        )))
    }
}

fn build_if_statement(node: &Node, source: &str) -> Result<IfStatement, ShellError> {
    // Simplified if statement parsing
    let mut condition = Vec::new();
    let mut then_branch = Vec::new();
    let mut else_branch = Vec::new();
    let mut in_condition = false;
    let mut in_then = false;
    let mut in_else = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "if" => in_condition = true,
            "then" => {
                in_condition = false;
                in_then = true;
            }
            "else" => {
                in_then = false;
                in_else = true;
            }
            "elif" => {
                // Treat elif as nested if in else
                in_then = false;
                in_else = true;
            }
            "fi" => break,
            _ => {
                if let Some(cmd) = build_command(&child, source)? {
                    if in_condition {
                        condition.push(cmd);
                    } else if in_then {
                        then_branch.push(cmd);
                    } else if in_else {
                        else_branch.push(cmd);
                    }
                }
            }
        }
    }

    Ok(IfStatement {
        condition,
        then_branch,
        else_branch: if else_branch.is_empty() {
            None
        } else {
            Some(else_branch)
        },
    })
}

fn build_while_statement(node: &Node, source: &str) -> Result<WhileStatement, ShellError> {
    let mut condition = Vec::new();
    let mut body = Vec::new();
    let mut in_condition = false;
    let mut in_body = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "while" | "until" => in_condition = true,
            "do" => {
                in_condition = false;
                in_body = true;
            }
            "done" => break,
            _ => {
                if let Some(cmd) = build_command(&child, source)? {
                    if in_condition {
                        condition.push(cmd);
                    } else if in_body {
                        body.push(cmd);
                    }
                }
            }
        }
    }

    Ok(WhileStatement { condition, body })
}

fn build_for_statement(node: &Node, source: &str) -> Result<ForStatement, ShellError> {
    let mut variable = String::new();
    let mut items = Vec::new();
    let mut body = Vec::new();
    let mut in_items = false;
    let mut in_body = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "variable_name" => {
                variable = node_text(&child, source);
            }
            "in" => in_items = true,
            "do" => {
                in_items = false;
                in_body = true;
            }
            "done" => break,
            "word" if in_items => {
                items.push(Word::Literal(node_text(&child, source)));
            }
            _ => {
                if in_body {
                    if let Some(cmd) = build_command(&child, source)? {
                        body.push(cmd);
                    }
                }
            }
        }
    }

    Ok(ForStatement {
        variable,
        items,
        body,
    })
}

fn node_text(node: &Node, source: &str) -> String {
    source[node.byte_range()].to_string()
}

fn extract_variable_name(node: &Node, source: &str) -> String {
    let text = node_text(node, source);
    // Remove ${ } or $ prefix
    text.trim_start_matches("${")
        .trim_start_matches('$')
        .trim_end_matches('}')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_with_args() {
        let mut parser = Parser::new().unwrap();
        let ast = parser.parse("ls | head -5").unwrap();

        // Check that we have a pipeline
        assert_eq!(ast.commands.len(), 1);
        if let Command::Pipeline(pipeline) = &ast.commands[0] {
            assert_eq!(pipeline.commands.len(), 2);

            // Check head command has -5 argument
            if let Command::Simple(head_cmd) = &pipeline.commands[1] {
                assert_eq!(head_cmd.name, "head");
                assert_eq!(head_cmd.args.len(), 1);
                assert_eq!(head_cmd.args[0].as_literal(), Some("-5"));
            } else {
                panic!("Expected simple command for head");
            }
        } else {
            panic!("Expected pipeline");
        }
    }
}
