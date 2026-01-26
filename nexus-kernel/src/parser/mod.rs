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
        "function_definition" => {
            let func_def = build_function_definition(node, source)?;
            Ok(Some(Command::Function(func_def)))
        }
        "case_statement" => {
            let case_stmt = build_case_statement(node, source)?;
            Ok(Some(Command::Case(case_stmt)))
        }
        "test_command" => {
            // [ expr ] or [[ expr ]] - convert to SimpleCommand
            let simple = build_test_command(node, source)?;
            Ok(Some(Command::Simple(simple)))
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
            "word" | "string" | "number" => {
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
    let mut condition = Vec::new();
    let mut then_branch = Vec::new();
    let mut elif_clauses: Vec<Node> = Vec::new();
    let mut final_else: Option<Vec<Command>> = None;
    let mut in_condition = false;
    let mut in_then = false;

    // First pass: collect all parts
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "if" => in_condition = true,
            "then" => {
                in_condition = false;
                in_then = true;
            }
            "elif_clause" => {
                elif_clauses.push(child);
                in_then = false;
            }
            "else_clause" => {
                final_else = Some(build_else_clause(&child, source)?);
                in_then = false;
            }
            "fi" => break,
            _ => {
                if let Some(cmd) = build_command(&child, source)? {
                    if in_condition {
                        condition.push(cmd);
                    } else if in_then {
                        then_branch.push(cmd);
                    }
                }
            }
        }
    }

    // Build the else branch by chaining elif clauses
    // Work backwards: start with final_else, then wrap each elif around it
    let mut else_branch = final_else;

    for elif_node in elif_clauses.into_iter().rev() {
        let elif_if = build_elif_clause_with_else(&elif_node, source, else_branch)?;
        else_branch = Some(vec![Command::If(elif_if)]);
    }

    Ok(IfStatement {
        condition,
        then_branch,
        else_branch,
    })
}

fn build_elif_clause_with_else(
    node: &Node,
    source: &str,
    else_branch: Option<Vec<Command>>,
) -> Result<IfStatement, ShellError> {
    let mut condition = Vec::new();
    let mut then_branch = Vec::new();
    let mut in_condition = false;
    let mut in_then = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "elif" => in_condition = true,
            "then" => {
                in_condition = false;
                in_then = true;
            }
            _ => {
                if let Some(cmd) = build_command(&child, source)? {
                    if in_condition {
                        condition.push(cmd);
                    } else if in_then {
                        then_branch.push(cmd);
                    }
                }
            }
        }
    }

    Ok(IfStatement {
        condition,
        then_branch,
        else_branch,
    })
}

fn build_else_clause(node: &Node, source: &str) -> Result<Vec<Command>, ShellError> {
    let mut commands = Vec::new();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "else" => continue,
            _ => {
                if let Some(cmd) = build_command(&child, source)? {
                    commands.push(cmd);
                }
            }
        }
    }

    Ok(commands)
}

fn build_test_command(node: &Node, source: &str) -> Result<SimpleCommand, ShellError> {
    // Convert [ expr ] or [[ expr ]] to a SimpleCommand
    // The command name is "[" and args are the expression tokens plus "]"
    let mut args = Vec::new();
    let mut name = "[".to_string();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "[" => name = "[".to_string(),
            "[[" => name = "[[".to_string(),
            "]" | "]]" => {
                // Closing bracket is added as final argument
                args.push(Word::Literal(child.kind().to_string()));
            }
            "binary_expression" | "unary_expression" | "string" | "word" | "number" => {
                // Extract all parts of the expression
                extract_test_expression(&child, source, &mut args);
            }
            "simple_expansion" | "expansion" => {
                args.push(Word::Variable(extract_variable_name(&child, source)));
            }
            "test_operator" => {
                args.push(Word::Literal(node_text(&child, source)));
            }
            _ => {
                // For any other child, try to extract its text
                let text = node_text(&child, source).trim().to_string();
                if !text.is_empty() && text != "[" && text != "]" {
                    args.push(Word::Literal(text));
                }
            }
        }
    }

    Ok(SimpleCommand {
        name,
        args,
        redirects: Vec::new(),
        env_assignments: Vec::new(),
    })
}

fn extract_test_expression(node: &Node, source: &str, args: &mut Vec<Word>) {
    // Recursively extract parts of test expressions
    match node.kind() {
        "binary_expression" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_test_expression(&child, source, args);
            }
        }
        "unary_expression" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_test_expression(&child, source, args);
            }
        }
        "simple_expansion" | "expansion" => {
            args.push(Word::Variable(extract_variable_name(node, source)));
        }
        "test_operator" | "=" | "!=" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge"
        | "-z" | "-n" | "-f" | "-d" | "-e" | "-r" | "-w" | "-x" | "-s" => {
            args.push(Word::Literal(node_text(node, source)));
        }
        "string" | "word" | "number" | "raw_string" => {
            args.push(Word::Literal(node_text(node, source)));
        }
        _ => {
            // For operators and other tokens, add them directly
            let text = node_text(node, source).trim().to_string();
            if !text.is_empty() {
                args.push(Word::Literal(text));
            }
        }
    }
}

fn build_while_statement(node: &Node, source: &str) -> Result<WhileStatement, ShellError> {
    let mut condition = Vec::new();
    let mut body = Vec::new();
    let mut in_condition = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "while" | "until" => in_condition = true,
            "do_group" => {
                in_condition = false;
                body = build_do_group(&child, source)?;
            }
            "do" | "done" | ";" => {
                in_condition = false;
            }
            _ => {
                if in_condition {
                    if let Some(cmd) = build_command(&child, source)? {
                        condition.push(cmd);
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

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "variable_name" => {
                variable = node_text(&child, source);
            }
            "in" => in_items = true,
            "do_group" => {
                // Parse body from do_group
                in_items = false;
                body = build_do_group(&child, source)?;
            }
            "word" | "number" | "string" | "raw_string" if in_items => {
                items.push(Word::Literal(node_text(&child, source)));
            }
            "simple_expansion" | "expansion" if in_items => {
                items.push(Word::Variable(extract_variable_name(&child, source)));
            }
            "concatenation" if in_items => {
                // Handle concatenated items
                items.push(Word::Literal(node_text(&child, source)));
            }
            "brace_expression" if in_items => {
                // Brace expansion like {1..5} or {a,b,c}
                items.push(Word::Literal(node_text(&child, source)));
            }
            ";" | "do" | "done" => {
                // Skip these tokens
                in_items = false;
            }
            _ => {}
        }
    }

    Ok(ForStatement {
        variable,
        items,
        body,
    })
}

fn build_do_group(node: &Node, source: &str) -> Result<Vec<Command>, ShellError> {
    let mut commands = Vec::new();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "do" | "done" | ";" => continue,
            _ => {
                if let Some(cmd) = build_command(&child, source)? {
                    commands.push(cmd);
                }
            }
        }
    }

    Ok(commands)
}

fn build_function_definition(node: &Node, source: &str) -> Result<FunctionDef, ShellError> {
    let mut name = String::new();
    let mut body = Vec::new();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "word" | "function_name" => {
                if name.is_empty() {
                    name = node_text(&child, source);
                }
            }
            "compound_statement" => {
                // Parse the body inside { }
                let mut body_cursor = child.walk();
                for body_child in child.children(&mut body_cursor) {
                    if let Some(cmd) = build_command(&body_child, source)? {
                        body.push(cmd);
                    }
                }
            }
            "subshell" => {
                // Some functions use ( ) instead of { }
                let mut body_cursor = child.walk();
                for body_child in child.children(&mut body_cursor) {
                    if let Some(cmd) = build_command(&body_child, source)? {
                        body.push(cmd);
                    }
                }
            }
            _ => {}
        }
    }

    Ok(FunctionDef { name, body })
}

fn build_case_statement(node: &Node, source: &str) -> Result<CaseStatement, ShellError> {
    let mut word = Word::Literal(String::new());
    let mut cases = Vec::new();
    let mut in_word = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "case" => in_word = true,
            "in" => in_word = false,
            "word" | "string" | "simple_expansion" | "expansion" if in_word => {
                word = if child.kind() == "simple_expansion" || child.kind() == "expansion" {
                    Word::Variable(extract_variable_name(&child, source))
                } else {
                    Word::Literal(node_text(&child, source))
                };
            }
            "case_item" => {
                let case_item = build_case_item(&child, source)?;
                cases.push(case_item);
            }
            "esac" => break,
            _ => {}
        }
    }

    Ok(CaseStatement { word, cases })
}

fn build_case_item(node: &Node, source: &str) -> Result<CaseItem, ShellError> {
    let mut patterns = Vec::new();
    let mut commands = Vec::new();
    let mut in_pattern = true;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "word" | "extglob_pattern" if in_pattern => {
                patterns.push(node_text(&child, source));
            }
            "|" => {} // Pattern separator, continue collecting patterns
            ")" => in_pattern = false,
            ";;" | ";&" | ";;&" => break,
            _ if !in_pattern => {
                if let Some(cmd) = build_command(&child, source)? {
                    commands.push(cmd);
                }
            }
            _ => {}
        }
    }

    Ok(CaseItem { patterns, commands })
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

    #[test]
    fn test_stdout_redirect() {
        let mut parser = Parser::new().unwrap();
        let ast = parser.parse("echo hello > output.txt").unwrap();

        assert_eq!(ast.commands.len(), 1);
        if let Command::Simple(cmd) = &ast.commands[0] {
            assert_eq!(cmd.name, "echo");
            assert_eq!(cmd.redirects.len(), 1);
            assert_eq!(cmd.redirects[0].fd, 1);
            assert_eq!(cmd.redirects[0].op, RedirectOp::Write);
            assert_eq!(cmd.redirects[0].target, "output.txt");
        } else {
            panic!("Expected simple command");
        }
    }

    #[test]
    fn test_stderr_redirect() {
        let mut parser = Parser::new().unwrap();
        let ast = parser.parse("cmd 2> error.txt").unwrap();

        assert_eq!(ast.commands.len(), 1);
        if let Command::Simple(cmd) = &ast.commands[0] {
            assert_eq!(cmd.name, "cmd");
            assert_eq!(cmd.redirects.len(), 1);
            assert_eq!(cmd.redirects[0].fd, 2);
            assert_eq!(cmd.redirects[0].op, RedirectOp::Write);
            assert_eq!(cmd.redirects[0].target, "error.txt");
        } else {
            panic!("Expected simple command");
        }
    }

    #[test]
    fn test_fd_duplication() {
        let mut parser = Parser::new().unwrap();
        let ast = parser.parse("cmd > out.txt 2>&1").unwrap();

        assert_eq!(ast.commands.len(), 1);
        if let Command::Simple(cmd) = &ast.commands[0] {
            assert_eq!(cmd.name, "cmd");
            assert_eq!(cmd.redirects.len(), 2, "Expected 2 redirects, got: {:?}", cmd.redirects);

            // First redirect: > out.txt
            assert_eq!(cmd.redirects[0].fd, 1);
            assert_eq!(cmd.redirects[0].op, RedirectOp::Write);
            assert_eq!(cmd.redirects[0].target, "out.txt");

            // Second redirect: 2>&1
            assert_eq!(cmd.redirects[1].fd, 2);
            assert_eq!(cmd.redirects[1].op, RedirectOp::DupWrite);
            assert_eq!(cmd.redirects[1].target, "1");
        } else {
            panic!("Expected simple command");
        }
    }

    #[test]
    fn test_input_redirect() {
        let mut parser = Parser::new().unwrap();
        let ast = parser.parse("cat < input.txt").unwrap();

        assert_eq!(ast.commands.len(), 1);
        if let Command::Simple(cmd) = &ast.commands[0] {
            assert_eq!(cmd.name, "cat");
            assert_eq!(cmd.redirects.len(), 1);
            assert_eq!(cmd.redirects[0].fd, 0);
            assert_eq!(cmd.redirects[0].op, RedirectOp::Read);
            assert_eq!(cmd.redirects[0].target, "input.txt");
        } else {
            panic!("Expected simple command");
        }
    }

    #[test]
    fn test_elif_parsing() {
        let mut parser = Parser::new().unwrap();
        let ast = parser.parse("if [ $x = 1 ]; then echo one; elif [ $x = 2 ]; then echo two; else echo other; fi").unwrap();

        assert_eq!(ast.commands.len(), 1);
        if let Command::If(if_stmt) = &ast.commands[0] {
            // Main condition should be [ $x = 1 ]
            assert_eq!(if_stmt.condition.len(), 1, "Expected 1 condition command");
            if let Command::Simple(cond) = &if_stmt.condition[0] {
                assert_eq!(cond.name, "[");
                eprintln!("Main condition args: {:?}", cond.args);
            }

            // Then branch should be echo one
            assert_eq!(if_stmt.then_branch.len(), 1, "Expected 1 then command");

            // Else branch should be a nested if (for elif)
            assert!(if_stmt.else_branch.is_some(), "Expected else branch");
            let else_branch = if_stmt.else_branch.as_ref().unwrap();
            assert_eq!(else_branch.len(), 1, "Expected 1 command in else branch (nested if)");

            if let Command::If(nested_if) = &else_branch[0] {
                // Nested if condition should be [ $x = 2 ]
                assert_eq!(nested_if.condition.len(), 1, "Expected 1 condition in elif");
                if let Command::Simple(elif_cond) = &nested_if.condition[0] {
                    assert_eq!(elif_cond.name, "[");
                    eprintln!("Elif condition args: {:?}", elif_cond.args);
                    // Should have: $x, =, 2, ]
                    assert!(elif_cond.args.len() >= 3, "Expected at least 3 args in elif condition");
                }

                // Nested else should be echo other
                assert!(nested_if.else_branch.is_some(), "Expected else in elif");
            } else {
                panic!("Expected nested If in else branch for elif");
            }
        } else {
            panic!("Expected If statement");
        }
    }

    #[test]
    fn test_for_parsing() {
        let mut parser = Parser::new().unwrap();
        let ast = parser.parse("for x in a b c; do echo $x; done").unwrap();

        assert_eq!(ast.commands.len(), 1);
        if let Command::For(for_stmt) = &ast.commands[0] {
            assert_eq!(for_stmt.variable, "x");
            eprintln!("For items: {:?}", for_stmt.items);
            assert_eq!(for_stmt.items.len(), 3, "Expected 3 items");
            assert_eq!(for_stmt.body.len(), 1, "Expected 1 command in body");
        } else {
            panic!("Expected For statement");
        }
    }

    #[test]
    fn test_for_numbers_parsing() {
        let mut parser = Parser::new().unwrap();
        let ast = parser.parse("for n in 1 2 3 4 5; do true; done").unwrap();

        assert_eq!(ast.commands.len(), 1);
        if let Command::For(for_stmt) = &ast.commands[0] {
            assert_eq!(for_stmt.variable, "n");
            assert_eq!(for_stmt.items.len(), 5, "Expected 5 items");
        } else {
            panic!("Expected For statement");
        }
    }

    #[test]
    fn test_for_brace_expansion_parsing() {
        let mut parser = Parser::new().unwrap();
        let ast = parser.parse("for i in {1..5}; do echo $i; done").unwrap();

        assert_eq!(ast.commands.len(), 1);
        if let Command::For(for_stmt) = &ast.commands[0] {
            assert_eq!(for_stmt.variable, "i");
            eprintln!("Brace expansion items: {:?}", for_stmt.items);
            // Should have 1 item: {1..5} which will be expanded later
            assert_eq!(for_stmt.items.len(), 1, "Expected 1 item (brace expr)");
            assert_eq!(for_stmt.items[0].as_literal(), Some("{1..5}"));
        } else {
            panic!("Expected For statement");
        }
    }

}
