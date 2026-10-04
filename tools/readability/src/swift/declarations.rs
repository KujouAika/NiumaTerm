use std::cmp::Ordering;

use tree_sitter::Node;

use crate::swift::{comment, finding, text};
use crate::{Category, Finding};

/// Import source categories in their required order. Apple frameworks and
/// third-party packages share the first group because an import names only a
/// module, and the module name does not show where it comes from.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ImportGroup {
    External,
    Project,
}

pub(super) fn import_group(node: Node, source: &str) -> ImportGroup {
    if import_path(node, source).starts_with("NiumaTerm") {
        ImportGroup::Project
    } else {
        ImportGroup::External
    }
}

/// Returns the imported module path, such as `Foundation.Date` for
/// `import struct Foundation.Date`. Attributes and the declaration kind do not
/// change the order.
fn import_path<'a>(node: Node, source: &'a str) -> &'a str {
    let mut cursor = node.walk();

    node.named_children(&mut cursor)
        .find(|child| child.kind() == "identifier")
        .map_or("", |path| text(path, source))
}

pub(super) fn inspect(root: Node, source: &str, lines: &[&str]) -> Vec<Finding> {
    let mut findings = Vec::new();

    imports(root, source, lines, &mut findings);
    visibility(root, lines, &mut findings);

    findings.sort_by_key(|finding| (finding.start.line, finding.start.column));

    findings
}

fn imports(root: Node, source: &str, lines: &[&str], findings: &mut Vec<Finding>) {
    let mut declaration = None;
    let mut highest: Option<Node> = None;
    let mut cursor = root.walk();

    for child in root.named_children(&mut cursor) {
        if comment(child) {
            continue;
        }

        // Each conditional compilation block is sorted on its own, because
        // its imports exist only in some builds.
        if child.kind() == "directive" {
            highest = None;

            continue;
        }

        if child.kind() != "import_declaration" {
            declaration.get_or_insert(child);

            continue;
        }

        if let Some(declaration) = declaration {
            findings.push(finding(
                Category::Declarations,
                "header",
                "move imports before other declarations",
                lines,
                &[child, declaration],
            ));
        }

        let violation = highest.and_then(|previous| {
            match import_group(child, source).cmp(&import_group(previous, source)) {
                Ordering::Less => Some((
                    "import-order",
                    "place NiumaTerm module imports after framework and package imports",
                )),
                Ordering::Equal if import_path(child, source) < import_path(previous, source) => {
                    Some((
                        "import-alphabetical",
                        "sort imports in each group alphabetically by module path",
                    ))
                }
                _ => None,
            }
        });

        match (violation, highest) {
            (Some((rule, message)), Some(previous)) => findings.push(finding(
                Category::Declarations,
                rule,
                message,
                lines,
                &[child, previous],
            )),
            _ => highest = Some(child),
        }
    }
}

/// Rejects `fileprivate` and `open`. A `private` member is visible to
/// extensions of its type in the same file, so `fileprivate` only adds access
/// from other types, which internal access also grants. The app exposes no
/// framework API for other modules to subclass.
fn visibility(root: Node, lines: &[&str], findings: &mut Vec<Finding>) {
    let mut cursor = root.walk();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        if node.kind() == "visibility_modifier" {
            let message = match node.child(0).map(|keyword| keyword.kind()) {
                Some("fileprivate") => {
                    Some("replace fileprivate with private, or omit it for internal access")
                }
                Some("open") => Some("replace open with public, or omit it for internal access"),
                _ => None,
            };

            if let Some(message) = message {
                findings.push(finding(
                    Category::Declarations,
                    "visibility",
                    message,
                    lines,
                    &[node],
                ));
            }
        }

        stack.extend(node.named_children(&mut cursor));
    }
}
