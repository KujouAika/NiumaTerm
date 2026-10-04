use tree_sitter::Node;

use crate::blank_lines::{BlankLines, Issues};
use crate::spacing::Options;
use crate::swift::declarations::import_group;
use crate::swift::{comment, end, multiline, position, text};

#[derive(PartialEq, Eq)]
enum CallKind {
    Function,
    Method,
}

/// A statement or declaration in a body, with whether a comment starts on a
/// line between it and the previous entry.
#[derive(Clone, Copy)]
struct Entry<'tree> {
    node: Node<'tree>,
    commented: bool,
}

struct Spacing<'a> {
    blank_lines: BlankLines<'a>,
    source: &'a str,
    options: &'a Options,
}

impl Spacing<'_> {
    fn visit(&mut self, node: Node) {
        match node.kind() {
            "statements" => self.statements(node),
            "source_file" => self.members(node, "items"),
            "class_body" | "enum_class_body" | "protocol_body" => self.members(node, "members"),
            "switch_statement" => self.switch_entries(node),
            _ => {}
        }

        let mut cursor = node.walk();

        for child in node.named_children(&mut cursor) {
            if !skips_formatting(child, self.source) {
                self.visit(child);
            }
        }
    }

    fn separate(&mut self, previous: Node, next: Node, rule: &'static str) {
        let end = position(&self.blank_lines.lines, end(previous));
        let start = position(&self.blank_lines.lines, next.start_position());

        self.blank_lines.separate(end, start, rule);
    }

    fn compact(&mut self, previous: Node, next: Node, rule: &'static str) {
        let end = position(&self.blank_lines.lines, end(previous));
        let start = position(&self.blank_lines.lines, next.start_position());

        self.blank_lines.compact(end, start, rule);
    }

    fn statements(&mut self, node: Node) {
        for (a, b) in pairs(node) {
            if let Some(rule) = boundary(a.node, b.node, self.source) {
                self.separate(a.node, b.node, rule);
            }
        }
    }

    fn members(&mut self, node: Node, rule: &'static str) {
        for (a, b) in pairs(node) {
            match (a.node.kind(), b.node.kind()) {
                ("import_declaration", "import_declaration") => {
                    if import_group(a.node, self.source) != import_group(b.node, self.source) {
                        self.separate(a.node, b.node, "import-groups");
                    } else {
                        self.compact(a.node, b.node, "import-blank-lines");
                    }
                }
                ("enum_entry", "enum_entry") => {
                    if !self.options.enum_variants {
                        self.compact(a.node, b.node, "enum-variant-blank-lines");
                    } else if a.commented || b.commented || multiline(a.node) || multiline(b.node) {
                        self.separate(a.node, b.node, "enum-variants");
                    }
                }
                _ if property(a.node) && property(b.node) => {
                    if a.commented || b.commented {
                        self.separate(a.node, b.node, "documented-properties");
                    }
                }
                _ => self.separate(a.node, b.node, rule),
            }
        }
    }

    fn switch_entries(&mut self, node: Node) {
        for (a, b) in pairs(node) {
            if a.node.kind() != "switch_entry" || b.node.kind() != "switch_entry" {
                continue;
            }

            if !self.options.match_arms {
                self.compact(a.node, b.node, "match-arm-blank-lines");
            } else if multiline(a.node) || multiline(b.node) {
                self.separate(a.node, b.node, "match-arms");
            }
        }
    }
}

/// Lists adjacent entries of a body. Compiler directives such as `#if` end a
/// run because the entries on either side may belong to different builds.
/// Nodes the parser could not recognize also end a run, since their extent
/// and kind are guesses.
fn pairs(node: Node) -> Vec<(Entry, Entry)> {
    let mut pairs = Vec::new();

    if node.is_error() {
        return pairs;
    }

    let mut previous: Option<Entry> = None;
    let mut last_row = None;
    let mut commented = false;
    let mut cursor = node.walk();

    for child in node.named_children(&mut cursor) {
        if comment(child) {
            commented |= last_row.is_none_or(|row| child.start_position().row > row);

            continue;
        }

        let entry = Entry {
            node: child,
            commented,
        };

        if child.kind() == "directive" || child.has_error() {
            previous = None;
        } else {
            if let Some(previous) = previous {
                pairs.push((previous, entry));
            }

            previous = Some(entry);
        }

        last_row = Some(end(child).row);
        commented = false;
    }

    pairs
}

/// Single-line stored and protocol properties may stay together, as struct
/// fields do. A property with a multiline initializer or accessor body reads
/// as a separate step and needs blank lines like a method.
fn property(node: Node) -> bool {
    matches!(
        node.kind(),
        "property_declaration" | "protocol_property_declaration"
    ) && !multiline(node)
}

fn skips_formatting(node: Node, source: &str) -> bool {
    node.prev_named_sibling().is_some_and(|previous| {
        comment(previous) && {
            let text = text(previous, source).trim_end();

            text == "// swift-format-ignore" || text.starts_with("// swift-format-ignore:")
        }
    })
}

fn unwrapped(node: Node) -> Node {
    match node.kind() {
        "try_expression" | "await_expression" => {
            node.child_by_field_name("expr").map_or(node, unwrapped)
        }
        _ => node,
    }
}

fn callee(node: Node) -> Option<Node> {
    (node.kind() == "call_expression")
        .then(|| node.named_child(0))
        .flatten()
}

fn flow(node: Node, source: &str) -> bool {
    let node = unwrapped(node);

    match node.kind() {
        "if_statement"
        | "guard_statement"
        | "switch_statement"
        | "for_statement"
        | "while_statement"
        | "repeat_while_statement"
        | "do_statement" => true,
        // The grammar parses `defer { ... }` as a call with a trailing closure.
        "call_expression" => callee(node).is_some_and(|callee| text(callee, source) == "defer"),
        "property_declaration" => node
            .child_by_field_name("value")
            .is_some_and(|value| flow(value, source)),
        _ => false,
    }
}

fn assertion(node: Node, source: &str) -> bool {
    let node = unwrapped(node);

    match node.kind() {
        "call_expression" => callee(node).is_some_and(|callee| {
            let name = text(callee, source);

            callee.kind() == "simple_identifier"
                && ["assert", "precondition", "XCTAssert"]
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
        }),
        "macro_invocation" => node
            .named_child(0)
            .is_some_and(|name| matches!(text(name, source), "expect" | "require")),
        _ => false,
    }
}

fn declaration(node: Node) -> bool {
    matches!(
        node.kind(),
        "function_declaration"
            | "class_declaration"
            | "protocol_declaration"
            | "typealias_declaration"
    )
}

/// Returns whether a local binding is mutable, or `None` for other
/// statements.
fn mutable_binding(node: Node, source: &str) -> Option<bool> {
    if node.kind() != "property_declaration" {
        return None;
    }

    let mut cursor = node.walk();

    let pattern = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == "value_binding_pattern")?;

    Some(
        pattern
            .child_by_field_name("mutability")
            .is_some_and(|keyword| text(keyword, source) == "var"),
    )
}

fn call_kind(node: Node) -> Option<CallKind> {
    callee(unwrapped(node)).map(|callee| match callee.kind() {
        "navigation_expression" => CallKind::Method,
        _ => CallKind::Function,
    })
}

fn boundary(a: Node, b: Node, source: &str) -> Option<&'static str> {
    if declaration(a) || declaration(b) {
        return Some("local-items");
    }

    if flow(a, source) || flow(b, source) {
        return Some("control-flow");
    }

    match (assertion(a, source), assertion(b, source)) {
        (true, true) => return None,
        (true, false) | (false, true) => return Some("assertions"),
        (false, false) => {}
    }

    if b.kind() == "control_transfer_statement" {
        return Some("result");
    }

    match (mutable_binding(a, source), mutable_binding(b, source)) {
        (Some(previous), Some(next)) => {
            if previous != next {
                return Some("binding-mutability");
            }

            if multiline(a) || multiline(b) {
                return Some("multiline-binding");
            }
        }
        (Some(_), None) | (None, Some(_)) => return Some("binding-and-action"),
        (None, None) => {}
    }

    if multiline(a) || multiline(b) {
        return Some("multiline-statement");
    }

    if call_kind(a) != call_kind(b) {
        Some("call-and-statement")
    } else {
        None
    }
}

pub(super) fn inspect(source: &str, root: Node, options: &Options) -> Issues {
    let mut spacing = Spacing {
        blank_lines: BlankLines::new(source),
        source,
        options,
    };

    spacing.visit(root);

    spacing.blank_lines.issues
}
