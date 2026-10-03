use crate::orchestration::template::{Segment, Template, TemplateError};

fn text(value: &str) -> Segment {
    Segment::Text(value.into())
}

#[test]
fn references_split_the_text_around_them() {
    let template = Template::parse("Plan:\n{{plan.output}}\nTask: {{ input }}.").unwrap();

    assert_eq!(
        template.segments(),
        [
            text("Plan:\n"),
            Segment::Output("plan".into()),
            text("\nTask: "),
            Segment::Input,
            text("."),
        ]
    );
    assert!(template.uses_input());
    assert_eq!(template.outputs().collect::<Vec<_>>(), ["plan"]);
}

#[test]
fn escaped_braces_and_lone_braces_stay_literal() {
    let template = Template::parse(r"fn main() { \{{x}} } \n {single}").unwrap();

    assert_eq!(
        template.segments(),
        [text(r"fn main() { {{x}} } \n {single}")]
    );
    assert!(!template.uses_input());
}

#[test]
fn text_with_multibyte_characters_is_kept_whole() {
    let template = Template::parse("café {{input}} naïve").unwrap();

    assert_eq!(
        template.segments(),
        [text("café "), Segment::Input, text(" naïve")]
    );
}

#[test]
fn malformed_references_are_errors() {
    assert_eq!(
        Template::parse("start {{plan.output"),
        Err(TemplateError::Unclosed(6))
    );

    for (source, inner) in [
        ("{{plan}}", "plan"),
        ("{{plan.text}}", "plan.text"),
        ("{{Plan.output}}", "Plan.output"),
        ("{{.output}}", ".output"),
        ("{{}}", ""),
    ] {
        assert_eq!(
            Template::parse(source),
            Err(TemplateError::UnknownReference(inner.into())),
            "{source}"
        );
    }
}

#[test]
fn renaming_a_reference_keeps_escaped_text() {
    let source = r"Plan: {{plan.output}} \{{plan.output}} {{ input }}";

    let mut template = Template::parse(source).unwrap();

    assert!(template.rename_output("plan", "outline"));
    assert!(!template.rename_output("missing", "other"));

    let rewritten = template.to_source();

    assert_eq!(
        rewritten,
        r"Plan: {{outline.output}} \{{plan.output}} {{input}}"
    );
    assert_eq!(Template::parse(&rewritten).unwrap(), template);
}
