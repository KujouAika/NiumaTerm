use crate::agent_tab::composer::{
    parse_annotated_prompt, prompt_with_response_annotations, restored_input_after_interruption,
};

#[test]
fn interrupted_prompt_returns_without_discarding_a_new_draft() {
    assert_eq!(
        restored_input_after_interruption("original prompt", ""),
        "original prompt"
    );
    assert_eq!(
        restored_input_after_interruption("original prompt", "new draft"),
        "original prompt\n\nnew draft"
    );
    assert_eq!(
        restored_input_after_interruption("original prompt", "original prompt"),
        "original prompt"
    );
}

#[test]
fn response_annotations_are_separate_from_the_visible_request() {
    let submitted = prompt_with_response_annotations(
        "Explain the difference",
        &["first excerpt".into(), "second\nexcerpt".into()],
    );

    let parsed = parse_annotated_prompt(&submitted).expect("annotated prompt");

    assert_eq!(parsed.prompt, "Explain the difference");
    assert_eq!(
        parsed
            .annotations
            .iter()
            .map(|annotation| annotation.text.as_str())
            .collect::<Vec<_>>(),
        ["first excerpt", "second\nexcerpt"]
    );
    assert!(submitted.contains("# Response annotations:"));
    assert!(!submitted.contains("# Selected text:"));

    let restored = parse_annotated_prompt(submitted.trim()).expect("restored annotated prompt");

    assert_eq!(restored.prompt, "Explain the difference");
    assert_eq!(restored.annotations.len(), 2);
    assert_eq!(
        prompt_with_response_annotations("unchanged", &[]),
        "unchanged"
    );
}
