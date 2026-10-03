//! Prompt templates: literal text with `{{input}}` and `{{<node>.output}}`
//! references.
//!
//! The language has no conditionals, loops or filters, so every reference a
//! template can make is visible to validation before a run starts. `\{{`
//! writes a literal `{{`; any other `{{...}}` is an error so that a typo in a
//! reference cannot reach the model as literal text.

#[cfg(test)]
#[path = "template_tests.rs"]
mod template_tests;

use std::mem;

use thiserror::Error;

const OPEN: &str = "{{";
const CLOSE: &str = "}}";
const ESCAPED_OPEN: &str = "\\{{";
const OUTPUT_FIELD: &str = ".output";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Input,
    Output(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Template {
    segments: Vec<Segment>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum TemplateError {
    #[error("`{{{{` at byte {0} has no closing `}}}}`")]
    Unclosed(usize),
    #[error("`{{{{{0}}}}}` is not `{{{{input}}}}` or `{{{{<node>.output}}}}`")]
    UnknownReference(String),
}

impl Template {
    pub fn parse(source: &str) -> Result<Self, TemplateError> {
        let mut segments = Vec::new();
        let mut text = String::new();
        let mut rest = source;

        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix(ESCAPED_OPEN) {
                text.push_str(OPEN);

                rest = after;
            } else if let Some(after) = rest.strip_prefix(OPEN) {
                let offset = source.len() - rest.len();
                let end = after.find(CLOSE).ok_or(TemplateError::Unclosed(offset))?;

                if !text.is_empty() {
                    segments.push(Segment::Text(mem::take(&mut text)));
                }

                segments.push(reference(after[..end].trim())?);

                rest = &after[end + CLOSE.len()..];
            } else {
                let next = rest
                    .char_indices()
                    .skip(1)
                    .map(|(index, _)| index)
                    .find(|&index| rest[index..].starts_with(['{', '\\']))
                    .unwrap_or(rest.len());

                text.push_str(&rest[..next]);

                rest = &rest[next..];
            }
        }

        if !text.is_empty() {
            segments.push(Segment::Text(text));
        }

        Ok(Self { segments })
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn uses_input(&self) -> bool {
        self.segments.contains(&Segment::Input)
    }

    /// The node ids whose outputs the template reads, in template order.
    pub fn outputs(&self) -> impl Iterator<Item = &str> {
        self.segments.iter().filter_map(|segment| match segment {
            Segment::Output(node) => Some(node.as_str()),
            _ => None,
        })
    }
}

fn reference(inner: &str) -> Result<Segment, TemplateError> {
    if inner == "input" {
        return Ok(Segment::Input);
    }

    match inner.strip_suffix(OUTPUT_FIELD) {
        Some(node) if is_node_id(node) => Ok(Segment::Output(node.to_owned())),
        _ => Err(TemplateError::UnknownReference(inner.to_owned())),
    }
}

/// Node ids are restricted so a reference can never be confused with
/// template syntax or with the field that follows it.
pub fn is_node_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}
