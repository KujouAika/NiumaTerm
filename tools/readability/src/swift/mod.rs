mod declarations;
mod expressions;
mod spacing;

#[cfg(test)]
mod tests;

use std::error::Error;

use proc_macro2::LineColumn;
use tree_sitter::{Node, Parser, Point, Tree};

use crate::blank_lines::{self, Issues};
use crate::{Category, Finding, Inspection, Options};

pub(crate) fn inspect(source: &str, options: &Options) -> Result<Inspection, Box<dyn Error>> {
    let tree = parse(source)?;
    let root = tree.root_node();
    let lines: Vec<&str> = source.lines().collect();

    let mut findings = declarations::inspect(root, source, &lines);

    if options.expressions.fixed_option_returns {
        findings.extend(expressions::inspect(root, &lines));
    }

    Ok(Inspection {
        spacing: spacing::inspect(source, root, &options.spacing),
        findings,
        unrecognized: first_error(root).map(|node| position(&lines, node.start_position())),
    })
}

pub(crate) fn apply(source: &str, issues: &Issues) -> Result<String, Box<dyn Error>> {
    let modified = blank_lines::edit(source, issues)?;

    let before = parse(source)?;
    let after = parse(&modified)?;

    if leaves(&before, source) != leaves(&after, &modified) {
        return Err("changing blank lines would change Swift tokens; file left untouched".into());
    }

    Ok(modified)
}

fn parse(source: &str) -> Result<Tree, Box<dyn Error>> {
    let mut parser = Parser::new();

    parser.set_language(&tree_sitter_swift::LANGUAGE.into())?;

    parser
        .parse(source, None)
        .ok_or_else(|| "the Swift parser returned no tree".into())
}

/// Lists every token's kind and text in source order. Whitespace is not a
/// token, so equal lists mean a blank-line edit changed only layout.
fn leaves<'a>(tree: &Tree, source: &'a str) -> Vec<(&'static str, &'a str)> {
    let mut leaves = Vec::new();
    let mut cursor = tree.walk();

    loop {
        let node = cursor.node();

        if node.child_count() == 0 {
            leaves.push((node.kind(), &source[node.byte_range()]));
        }

        if cursor.goto_first_child() || cursor.goto_next_sibling() {
            continue;
        }

        loop {
            if !cursor.goto_parent() {
                return leaves;
            }

            if cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Returns the innermost unrecognized node. Error recovery can wrap a whole
/// file in an error node, so the outermost one would not locate the cause.
fn first_error(node: Node) -> Option<Node> {
    if !node.has_error() {
        return None;
    }

    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find_map(first_error)
        .or_else(|| (node.is_error() || node.is_missing()).then_some(node))
}

/// Converts a tree-sitter byte column into the character column used by
/// `BlankLines` and diagnostics.
fn position(lines: &[&str], point: Point) -> LineColumn {
    let column = lines.get(point.row).map_or(point.column, |line| {
        line.char_indices()
            .take_while(|(offset, _)| *offset < point.column)
            .count()
    });

    LineColumn {
        line: point.row + 1,
        column,
    }
}

/// Returns where the node's last code token ends. The grammar extends some
/// nodes, such as `statements` and `switch_entry`, over the whitespace before
/// the next token, and attaches comments that precede a following sibling.
/// Neither belongs to the node's own layout.
fn end(node: Node) -> Point {
    last_token_end(node).unwrap_or_else(|| node.end_position())
}

fn last_token_end(node: Node) -> Option<Point> {
    if node.child_count() == 0 {
        return (node.start_byte() < node.end_byte() && !comment(node))
            .then(|| node.end_position());
    }

    let mut cursor = node.walk();

    let children: Vec<_> = node.children(&mut cursor).collect();

    children.into_iter().rev().find_map(last_token_end)
}

fn comment(node: Node) -> bool {
    matches!(node.kind(), "comment" | "multiline_comment")
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

fn multiline(node: Node) -> bool {
    end(node).row > node.start_position().row
}

fn finding(
    category: Category,
    rule: &'static str,
    message: &'static str,
    lines: &[&str],
    nodes: &[Node],
) -> Finding {
    Finding {
        category,
        rule,
        message,
        start: position(lines, nodes[0].start_position()),
        lines: nodes
            .iter()
            .map(|node| (node.start_position().row + 1, end(*node).row + 1))
            .collect(),
    }
}
