use crate::agent_tab::transcript::reveal::*;

/// Only a finished exit reports itself, and only an exit does: the content of
/// an open disclosure has to survive its own entrance.
#[test]
fn only_a_finished_exit_asks_to_be_taken_down() {
    let mut reveals = Reveals::default();

    let start = Instant::now();

    reveals.open(RevealKey::Row(1), start);

    reveals.close(RevealKey::Group(0), start);

    assert!(reveals.shut(start).is_empty());
    assert_eq!(
        reveals.shut(start + DISMISS_DURATION),
        vec![RevealKey::Group(0)]
    );
}

/// The exit runs over a span of its own, short enough that the space comes
/// back before an entrance would be half over. Content that leaves by fading
/// rather than by height holds its full height for the whole exit, so an exit
/// as long as an entrance leaves the reader watching nothing move: what they
/// clicked for is the space closing up, and that only happens here.
#[test]
fn an_exit_hands_the_space_back_inside_an_entrance() {
    let mut reveals = Reveals::default();

    let start = Instant::now();

    reveals.close(RevealKey::Annotation(4), start);

    assert!(reveals.shut(start).is_empty());
    assert_eq!(
        reveals.shut(start + REVEAL_DURATION / 2),
        vec![RevealKey::Annotation(4)]
    );
}
