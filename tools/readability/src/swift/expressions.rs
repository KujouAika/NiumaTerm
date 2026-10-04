use tree_sitter::Node;

use crate::swift::{comment, finding};
use crate::{Category, Finding};

/// Reports functions declared to return an optional whose every return path
/// produces `nil`. Swift wraps non-optional values implicitly, so a fixed
/// `.some` result cannot be recognized without types; only the `nil` case is
/// checked.
pub(super) fn inspect(root: Node, lines: &[&str]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut cursor = root.walk();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        if node.kind() == "function_declaration" && fixed_nil(node) {
            findings.push(finding(
                Category::Expressions,
                "fixed-option-return",
                "this function only returns nil; review its return value and callers",
                lines,
                &[node],
            ));
        }

        stack.extend(node.named_children(&mut cursor));
    }

    findings.sort_by_key(|finding| (finding.start.line, finding.start.column));

    findings
}

fn fixed_nil(function: Node) -> bool {
    let mut cursor = function.walk();
    let mut children = function.children(&mut cursor);

    let optional = children
        .by_ref()
        .any(|child| child.kind() == "->")
        .then(|| children.find(|child| child.is_named()))
        .flatten()
        .is_some_and(|returned| returned.kind() == "optional_type");

    let Some(statements) = function.child_by_field_name("body").and_then(|body| {
        let mut cursor = body.walk();

        body.named_children(&mut cursor)
            .find(|child| child.kind() == "statements")
    }) else {
        return false;
    };

    let mut cursor = statements.walk();

    let code: Vec<_> = statements
        .children(&mut cursor)
        .filter(|child| !comment(*child))
        .collect();

    // A single-expression body returns its value implicitly. In longer
    // bodies a final expression is discarded, so only `return nil` counts.
    let ends_in_nil = match code.as_slice() {
        [only] if only.kind() == "nil" => true,
        [.., last] => returns_nil(*last),
        [] => false,
    };

    let mut returns = Vec::new();

    collect_returns(statements, &mut returns);

    optional && ends_in_nil && returns.into_iter().all(returns_nil)
}

fn returns_nil(node: Node) -> bool {
    node.kind() == "control_transfer_statement"
        && node
            .child(0)
            .is_some_and(|keyword| keyword.kind() == "return")
        && node
            .child_by_field_name("result")
            .is_some_and(|result| result.kind() == "nil")
}

/// Collects `return` statements of the enclosing function. Closures, nested
/// functions, and local types return from their own scopes.
fn collect_returns<'tree>(node: Node<'tree>, returns: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();

    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "lambda_literal"
            | "function_declaration"
            | "class_declaration"
            | "protocol_declaration" => {}
            "control_transfer_statement" => {
                if child
                    .child(0)
                    .is_some_and(|keyword| keyword.kind() == "return")
                {
                    returns.push(child);
                }
            }
            _ => collect_returns(child, returns),
        }
    }
}
