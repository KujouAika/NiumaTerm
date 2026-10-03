//! The text sent for one node.

#[cfg(test)]
#[path = "compose_tests.rs"]
mod compose_tests;

use crate::orchestration::graph::Graph;
use crate::orchestration::template::Segment;

/// Build the text sent for `node`. `outputs[i]` holds node `i`'s output once
/// it has completed. `role` is the slot's role when this node opens its
/// slot's conversation, and `None` for every later node in that
/// conversation, which already contains the role.
///
/// Returns `None` when an output the text needs is missing; the scheduler
/// only composes a node after all of its ancestors have completed.
pub fn compose(
    graph: &Graph,
    node: usize,
    input: &str,
    role: Option<&str>,
    outputs: &[Option<String>],
) -> Option<String> {
    let output = |index: usize| outputs.get(index)?.as_deref();

    let body = match graph.template(node) {
        Some(template) => {
            let mut text = String::new();

            for segment in template.segments() {
                match segment {
                    Segment::Text(literal) => text.push_str(literal),
                    Segment::Input => text.push_str(input),
                    Segment::Output(id) => text.push_str(output(graph.node_index(id)?)?),
                }
            }

            text
        }
        None if graph.needs(node).is_empty() => input.to_owned(),
        None => graph
            .needs(node)
            .iter()
            .map(|&dependency| {
                let id = &graph.definition().nodes[dependency].id;

                Some(format!("## {id}\n\n{}", output(dependency)?))
            })
            .collect::<Option<Vec<_>>>()?
            .join("\n\n"),
    };

    Some(match role.filter(|role| !role.is_empty()) {
        Some(role) => format!("{role}\n\n{body}"),
        None => body,
    })
}
