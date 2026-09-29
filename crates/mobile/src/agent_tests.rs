use nmt_agent::chat::{Question, QuestionInput, QuestionOption};

use crate::agent::draft_answers;
use crate::records::QuestionAnswer;

fn question(multi_select: bool, input: QuestionInput) -> Question {
    Question {
        header: None,
        question: "Which?".into(),
        multi_select,
        options: ["a", "b", "c"]
            .into_iter()
            .map(|label| QuestionOption {
                label: label.into(),
                description: None,
            })
            .collect(),
        input,
    }
}

fn answer(selected: &[u32], text: Option<&str>) -> QuestionAnswer {
    QuestionAnswer {
        selected: selected.to_vec(),
        text: text.map(str::to_owned),
    }
}

#[test]
fn answers_take_the_shape_each_question_allows() {
    let questions = [
        question(false, QuestionInput::SelectionOnly),
        question(true, QuestionInput::SelectionOnly),
        question(false, QuestionInput::Text),
    ];

    let draft = draft_answers(
        &questions,
        vec![
            // One choice keeps one pick, and a question without text
            // ignores typed text.
            answer(&[2, 0], Some("typed")),
            // Out-of-range and repeated picks are dropped.
            answer(&[2, 9, 0, 2], None),
            answer(&[1], Some("my own")),
        ],
    )
    .unwrap();

    assert_eq!(draft.selected, [vec![2], vec![0, 2], vec![1]]);
    assert_eq!(draft.custom, [false, false, true]);
    assert_eq!(draft.text, ["", "", "my own"]);
}

#[test]
fn answers_for_a_different_number_of_questions_are_refused() {
    let questions = [question(false, QuestionInput::SelectionOnly)];

    assert!(draft_answers(&questions, Vec::new()).is_none());
}
