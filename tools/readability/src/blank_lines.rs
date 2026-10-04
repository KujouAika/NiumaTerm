use std::collections::BTreeMap;
use std::error::Error;

use proc_macro2::LineColumn;

pub(crate) type Issues = BTreeMap<usize, Issue>;

pub(crate) struct Issue {
    pub(crate) rule: &'static str,
    pub(crate) through_line: usize,
    pub(crate) edit: BlankLineEdit,
}

#[derive(Clone, Copy)]
pub(crate) enum BlankLineEdit {
    Insert,
    Remove,
}

impl Issue {
    pub(crate) fn message(&self) -> &'static str {
        match self.edit {
            BlankLineEdit::Insert => "missing blank line",
            BlankLineEdit::Remove => "unexpected blank line",
        }
    }
}

/// Collects blank-line edits between syntax nodes. Positions use 1-based
/// lines and 0-based character columns so Rust spans and converted Swift
/// positions address the same text.
pub(crate) struct BlankLines<'a> {
    pub(crate) lines: Vec<&'a str>,
    pub(crate) issues: Issues,
}

impl<'a> BlankLines<'a> {
    pub(crate) fn new(source: &'a str) -> Self {
        Self {
            lines: source.lines().collect(),
            issues: BTreeMap::new(),
        }
    }

    pub(crate) fn separate(&mut self, end: LineColumn, start: LineColumn, reason: &'static str) {
        if start.line <= end.line {
            return;
        }

        // A trailing block comment can extend across the insertion position.
        // Leave that boundary alone so its text remains unchanged.
        if self.lines[end.line - 1]
            .chars()
            .skip(end.column)
            .collect::<String>()
            .contains("/*")
        {
            return;
        }

        // Comments between adjacent statements explain the following step.
        // Insert before the comment, leaving it attached to that statement.
        let gap = &self.lines[end.line..start.line - 1];

        if gap.iter().any(|line| line.trim().is_empty()) {
            return;
        }

        self.issues.entry(end.line).or_insert(Issue {
            rule: reason,
            through_line: start.line - 1,
            edit: BlankLineEdit::Insert,
        });
    }

    pub(crate) fn compact(&mut self, end: LineColumn, start: LineColumn, rule: &'static str) {
        let mut comment_depth = 0;

        // Only the gap is scanned, so literal contents inside either item stay
        // untouched. Block comments can start on the previous item's last line
        // and contain nested comments and blank lines that must be preserved.
        for index in end.line - 1..start.line.saturating_sub(1) {
            let line = self.lines[index];

            if index >= end.line && comment_depth == 0 && line.trim().is_empty() {
                self.issues.insert(
                    index,
                    Issue {
                        rule,
                        through_line: index + 1,
                        edit: BlankLineEdit::Remove,
                    },
                );
            }

            let mut bytes = if index == end.line - 1 {
                let offset = line
                    .char_indices()
                    .nth(end.column)
                    .map_or(line.len(), |(offset, _)| offset);

                &line.as_bytes()[offset..]
            } else {
                line.as_bytes()
            };

            while bytes.len() >= 2 {
                match &bytes[..2] {
                    b"//" if comment_depth == 0 => break,
                    b"/*" => {
                        comment_depth += 1;
                        bytes = &bytes[2..];
                    }
                    b"*/" if comment_depth > 0 => {
                        comment_depth -= 1;
                        bytes = &bytes[2..];
                    }
                    _ => bytes = &bytes[1..],
                }
            }
        }
    }
}

/// Inserts and removes the blank lines named by `issues`, keeping each
/// inserted line's newline style equal to the line before it. Callers compare
/// the result's tokens with the original before writing it.
pub(crate) fn edit(source: &str, issues: &Issues) -> Result<String, Box<dyn Error>> {
    let mut modified = String::with_capacity(source.len() + issues.len() * 2);
    let mut newline = "\n";

    for (index, line) in source.split_inclusive('\n').enumerate() {
        match issues.get(&index).map(|issue| issue.edit) {
            Some(BlankLineEdit::Insert) => modified.push_str(newline),
            Some(BlankLineEdit::Remove) => {
                if !line.trim().is_empty() {
                    return Err("cannot remove a nonblank line; file left untouched".into());
                }

                continue;
            }
            None => {}
        }

        modified.push_str(line);

        newline = if line.ends_with("\r\n") { "\r\n" } else { "\n" };
    }

    Ok(modified)
}
