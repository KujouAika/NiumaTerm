use std::collections::BTreeMap;

use crate::blank_lines::{BlankLineEdit, Issue};
use crate::swift::{apply, inspect};
use crate::{Inspection, Options, expressions, spacing};

fn check(source: &str) -> Inspection {
    inspect(source, &Options::default()).unwrap()
}

fn spacing_rules(inspection: &Inspection) -> Vec<(usize, &'static str)> {
    inspection
        .spacing
        .iter()
        .map(|(line, issue)| (line + 1, issue.rule))
        .collect()
}

fn finding_rules(inspection: &Inspection) -> Vec<(usize, &'static str)> {
    inspection
        .findings
        .iter()
        .map(|finding| (finding.start.line, finding.rule))
        .collect()
}

fn fixed(source: &str) -> String {
    apply(source, &check(source).spacing).unwrap()
}

fn assert_fixes(source: &str, expected: &str) {
    assert_eq!(fixed(source), expected);
    assert!(check(expected).spacing.is_empty(), "{expected}");
}

#[test]
fn separates_bindings_control_flow_and_results() {
    let source = "func read() -> Int {\n    let ready = true\n    guard ready else { return 0 }\n    consume()\n    return 1\n}\n";

    assert_fixes(
        source,
        "func read() -> Int {\n    let ready = true\n\n    guard ready else { return 0 }\n\n    consume()\n\n    return 1\n}\n",
    );

    assert_eq!(
        spacing_rules(&check(source)),
        [(3, "control-flow"), (4, "control-flow"), (5, "result")]
    );
}

#[test]
fn separates_binding_mutability_and_multiline_bindings() {
    let source = "func run() {\n    let first = 1\n    let second = 2\n    var count = 0\n    var total = 0\n    let items = values\n        .map { $0 }\n    let last = 3\n}\n";

    assert_eq!(
        spacing_rules(&check(source)),
        [
            (4, "binding-mutability"),
            (6, "binding-mutability"),
            (8, "multiline-binding"),
        ]
    );
}

#[test]
fn separates_function_calls_method_calls_and_assignments() {
    let source = "func run() {\n    prepare()\n    flush()\n    self.update()\n    tasks.insert(key)\n    try await store.save()\n    count += 1\n    total = count\n}\n";

    assert_fixes(
        source,
        "func run() {\n    prepare()\n    flush()\n\n    self.update()\n    tasks.insert(key)\n    try await store.save()\n\n    count += 1\n    total = count\n}\n",
    );
}

#[test]
fn groups_assertions_and_separates_them_from_other_statements() {
    let source = "func verify() {\n    let value = make()\n    assert(value > 0)\n    precondition(ready)\n    #expect(value == 1)\n    XCTAssertEqual(value, 1)\n    consume(value)\n}\n";

    assert_eq!(
        spacing_rules(&check(source)),
        [(3, "assertions"), (7, "assertions")]
    );
}

#[test]
fn treats_defer_and_conditional_bindings_as_control_flow() {
    let source = "func run() {\n    open()\n    defer { close() }\n    let value = if ready { 1 } else { 2 }\n    use(value)\n}\n";

    assert_eq!(
        spacing_rules(&check(source)),
        [
            (3, "control-flow"),
            (4, "control-flow"),
            (5, "control-flow")
        ]
    );
}

#[test]
fn separates_local_declarations() {
    let source =
        "func run() {\n    var path = Path()\n    func corner() { path.move() }\n    corner()\n}\n";

    assert_eq!(
        spacing_rules(&check(source)),
        [(3, "local-items"), (4, "local-items")]
    );
}

#[test]
fn removes_blank_lines_between_switch_cases_unless_enabled_otherwise() {
    let source = "func run() {\n    switch value {\n    case 0:\n        first()\n        second()\n\n    // The fallback.\n\n    default:\n        third()\n    }\n}\n";

    assert_fixes(
        source,
        "func run() {\n    switch value {\n    case 0:\n        first()\n        second()\n    // The fallback.\n    default:\n        third()\n    }\n}\n",
    );

    let options = Options {
        spacing: spacing::Options {
            match_arms: true,
            enum_variants: false,
        },
        ..Options::default()
    };

    let compact = "func run() {\n    switch value {\n    case 0:\n        first()\n        second()\n    default:\n        third()\n    }\n}\n";
    let inspection = inspect(compact, &options).unwrap();

    assert_eq!(spacing_rules(&inspection), [(6, "match-arms")]);
}

#[test]
fn removes_blank_lines_between_enum_cases_unless_enabled_otherwise() {
    let source = "enum Kind {\n    case first\n\n    /// The second case.\n    case second(Int)\n\n    case third\n\n    func describe() {}\n}\n";

    assert_fixes(
        source,
        "enum Kind {\n    case first\n    /// The second case.\n    case second(Int)\n    case third\n\n    func describe() {}\n}\n",
    );

    let options = Options {
        spacing: spacing::Options {
            match_arms: false,
            enum_variants: true,
        },
        ..Options::default()
    };

    let compact = "enum Kind {\n    case first\n    /// The second case.\n    case second(Int)\n    case third\n}\n";
    let inspection = inspect(compact, &options).unwrap();

    assert_eq!(
        spacing_rules(&inspection),
        [(3, "enum-variants"), (5, "enum-variants")]
    );
}

#[test]
fn keeps_single_line_properties_together_and_separates_other_members() {
    let source = "struct Row {\n    let id: Int\n    var title = \"\"\n    /// Rows above this one.\n    var depth: Int\n    var body: some View {\n        Text(title)\n    }\n    init() {}\n}\n";

    assert_fixes(
        source,
        "struct Row {\n    let id: Int\n    var title = \"\"\n\n    /// Rows above this one.\n    var depth: Int\n\n    var body: some View {\n        Text(title)\n    }\n\n    init() {}\n}\n",
    );

    assert_eq!(
        spacing_rules(&check(source)),
        [(4, "documented-properties"), (6, "members"), (9, "members")]
    );
}

#[test]
fn separates_top_level_items() {
    let source = "import SwiftUI\nstruct First {}\nstruct Second {}\n";

    assert_eq!(spacing_rules(&check(source)), [(2, "items"), (3, "items")]);
}

#[test]
fn groups_imports_by_source() {
    let source = "import SwiftUI\n\nimport UIKit\nimport NiumaTermCore\n\nstruct View {}\n";

    assert_fixes(
        source,
        "import SwiftUI\nimport UIKit\n\nimport NiumaTermCore\n\nstruct View {}\n",
    );

    assert_eq!(
        spacing_rules(&check(source)),
        [(2, "import-blank-lines"), (4, "import-groups")]
    );
}

#[test]
fn reports_import_order_position_and_alphabetical_order() {
    let source = "import NiumaTermCore\n\nimport SwiftUI\nimport Foundation\n@preconcurrency import struct Combine.Published\n\nstruct View {}\n\nimport os\n";

    assert_eq!(
        finding_rules(&check(source)),
        [
            (3, "import-order"),
            (4, "import-order"),
            (5, "import-order"),
            (9, "header"),
            (9, "import-order"),
        ]
    );

    let sorted =
        "import Foundation\nimport SwiftUI\nimport os\nimport Combine\n\nimport NiumaTermCore\n";

    assert_eq!(finding_rules(&check(sorted)), [(4, "import-alphabetical")]);
}

#[test]
fn sorts_imports_within_each_conditional_block() {
    let source = "import SwiftUI\n#if canImport(UIKit)\nimport UIKit\n#endif\nimport Foundation\n";

    assert!(finding_rules(&check(source)).is_empty());
}

#[test]
fn rejects_fileprivate_and_open_visibility() {
    let source = "open class Base {\n    fileprivate func tick() {}\n    private(set) var count = 0\n    fileprivate(set) var total = 0\n    public func run() {}\n}\n";

    assert_eq!(
        finding_rules(&check(source)),
        [(1, "visibility"), (2, "visibility"), (4, "visibility")]
    );
}

#[test]
fn does_not_pair_entries_across_directives() {
    let source = "func run() {\n    #if DEBUG\n    log()\n    #endif\n    let value = 1\n}\n";

    assert!(check(source).spacing.is_empty());
}

#[test]
fn skips_spacing_inside_declarations_marked_for_swift_format_to_ignore() {
    let source = "// swift-format-ignore\nfunc compact() {\n    let value = 1\n    consume(value)\n}\n\nfunc spaced() {\n    let value = 1\n    consume(value)\n}\n";

    assert_eq!(spacing_rules(&check(source)), [(9, "binding-and-action")]);
}

#[test]
fn inserts_blank_lines_before_comments_and_after_trailing_comments() {
    let source = "func run() {\n    let value = 1 // The start.\n    // Consume it.\n    consume(value)\n}\n";

    assert_fixes(
        source,
        "func run() {\n    let value = 1 // The start.\n\n    // Consume it.\n    consume(value)\n}\n",
    );
}

#[test]
fn preserves_crlf_line_endings() {
    let source = "func run() {\r\n    let value = 1\r\n    consume(value)\r\n}\r\n";

    assert_eq!(
        fixed(source),
        "func run() {\r\n    let value = 1\r\n\r\n    consume(value)\r\n}\r\n"
    );
}

#[test]
fn refuses_edits_that_change_tokens() {
    let source = "let text = \"\"\"\n    first\n    second\n    \"\"\"\n";

    let issues = BTreeMap::from([(
        2,
        Issue {
            rule: "test",
            through_line: 2,
            edit: BlankLineEdit::Insert,
        },
    )]);

    assert!(apply(source, &issues).is_err());
}

#[test]
fn skips_unrecognized_syntax_and_reports_its_position() {
    let source = "func run() async {\n    if try await !handle.ok() {}\n    let value = 1\n    consume(value)\n}\n\nfunc other() {\n    let value = 1\n    consume(value)\n}\n";
    let inspection = check(source);

    assert!(inspection.unrecognized.is_some());
    assert!(
        spacing_rules(&inspection).contains(&(9, "binding-and-action")),
        "{:?}",
        spacing_rules(&inspection)
    );
}

#[test]
fn reports_fixed_nil_returns_only_when_enabled() {
    let source = "func none() -> Int? {\n    guard ready else { return nil }\n    let values = items.map { item -> Int? in return item }\n    return nil\n}\n\nfunc implicit() -> String? { nil }\n\nfunc some() -> Int? {\n    guard ready else { return nil }\n    return 1\n}\n\nfunc plain() -> Int {\n    return 0\n}\n";

    assert!(check(source).findings.is_empty());

    let options = Options {
        expressions: expressions::Options {
            fixed_option_returns: true,
        },
        ..Options::default()
    };

    let inspection = inspect(source, &options).unwrap();

    assert_eq!(
        finding_rules(&inspection),
        [(1, "fixed-option-return"), (7, "fixed-option-return")]
    );
}
