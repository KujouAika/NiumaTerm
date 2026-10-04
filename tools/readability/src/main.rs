mod blank_lines;
mod declarations;
mod expressions;
mod spacing;
mod swift;

use std::collections::BTreeSet;
use std::error::Error;
use std::io::Write;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{env, fs};

use proc_macro2::LineColumn;
use tempfile::NamedTempFile;

use crate::blank_lines::Issues;

const HELP: &str = "Rust and Swift readability checks

Usage: cargo readability --check [--enable RULE ...] [PATH ...]
       cargo readability --fix [--enable RULE ...] [PATH ...]
       cargo readability --staged [--enable RULE ...]

Without paths, --check and --fix scan tracked and unignored Rust files in crates/
and Swift files in mobile/ios/.
Explicit paths are relative to the current directory and may name directories.
--staged checks changed boundaries and declarations in the same staged files.
--fix adds or removes blank lines in working files; it never stages changes.
Declaration position, order, visibility, and attribute issues require manual edits.
Immediately calling an anonymous Rust closure is forbidden and requires a manual
rewrite.
--enable RULE opts into spacing/match-arms, spacing/enum-variants, or
expressions/fixed-option-return; repeat to enable multiple rules.
By default, blank lines between match arms and enum variants are forbidden.
Swift switch cases and enum cases follow the match arm and enum variant rules.
Blank lines within module declaration groups and import groups are also forbidden.
Enabling either optional spacing rule replaces its corresponding default rule.
Exit codes: 0 clean or fixed, 1 readability issues, 2 invalid input or tool failure.";

#[derive(Clone, Copy)]
enum Language {
    Rust,
    Swift,
}

#[derive(Default)]
pub(crate) struct Options {
    pub(crate) spacing: spacing::Options,
    pub(crate) expressions: expressions::Options,
}

#[derive(Clone, Copy)]
pub(crate) enum Category {
    Declarations,
    Expressions,
}

/// A declaration or expression diagnostic. `lines` holds the 1-based line
/// ranges whose staged edits make the diagnostic relevant to a commit.
pub(crate) struct Finding {
    pub(crate) category: Category,
    pub(crate) rule: &'static str,
    pub(crate) message: &'static str,
    pub(crate) start: LineColumn,
    pub(crate) lines: Vec<(usize, usize)>,
}

pub(crate) struct Inspection {
    pub(crate) spacing: Issues,
    pub(crate) findings: Vec<Finding>,

    /// The first position the Swift parser could not recognize. Checks skip
    /// the affected nodes, so valid code the grammar does not support yet
    /// cannot block a commit.
    pub(crate) unrecognized: Option<LineColumn>,
}

#[derive(Default)]
struct Totals {
    spacing: usize,
    declarations: usize,
    expressions: usize,
}

fn main() -> ExitCode {
    match run() {
        Ok(clean) => ExitCode::from(u8::from(!clean)),
        Err(error) => {
            eprintln!("readability: {error}");

            ExitCode::from(2)
        }
    }
}

fn run() -> Result<bool, Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);

    let mode = arguments.next().unwrap_or_default();

    if mode == "--help" || mode == "-h" {
        println!("{HELP}");

        return Ok(true);
    }

    if mode != "--check" && mode != "--fix" && mode != "--staged" {
        return Err(HELP.into());
    }

    let mut paths = Vec::new();
    let mut options = Options::default();

    while let Some(argument) = arguments.next() {
        if argument == "--enable" {
            let rule = arguments.next().ok_or("--enable requires a rule name")?;

            match rule.to_str() {
                Some("spacing/match-arms") => options.spacing.match_arms = true,
                Some("spacing/enum-variants") => options.spacing.enum_variants = true,
                Some("expressions/fixed-option-return") => {
                    options.expressions.fixed_option_returns = true;
                }
                _ => {
                    return Err(format!("unknown optional rule: {}", rule.to_string_lossy()).into());
                }
            }
        } else {
            paths.push(PathBuf::from(argument));
        }
    }

    if mode == "--staged" && !paths.is_empty() {
        return Err("--staged cannot be combined with paths or other modes".into());
    }

    if paths
        .iter()
        .any(|path| path.as_os_str().to_string_lossy().starts_with('-'))
    {
        return Err("unexpected option; choose one of --check, --fix, or --staged".into());
    }

    let directory = env::current_dir()?;

    let mut files = BTreeSet::new();
    let mut staged = Vec::new();

    if mode == "--staged" || paths.is_empty() {
        let root = git(&directory, &["rev-parse", "--show-toplevel"])?;
        let root = PathBuf::from(root.trim_end_matches(['\r', '\n']));

        env::set_current_dir(&root)?;

        if mode == "--staged" {
            let names = git(
                &root,
                &[
                    "diff",
                    "--cached",
                    "--name-only",
                    "-z",
                    "--no-renames",
                    "--diff-filter=ACM",
                    "--",
                    "crates/",
                    "mobile/ios/",
                ],
            )?;

            for name in names.split('\0') {
                let Some(language) = tracked_language(name) else {
                    continue;
                };

                let source = git(&root, &["show", &format!(":{name}")])?;

                let diff = git(
                    &root,
                    &[
                        "diff",
                        "--cached",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--no-renames",
                        "--unified=0",
                        "--inter-hunk-context=0",
                        "--no-color",
                        "--text",
                        "--",
                        name,
                    ],
                )?;

                staged.push((
                    PathBuf::from(name),
                    language,
                    source,
                    changed_boundaries(&diff)?,
                ));
            }
        } else {
            let names = git(
                &root,
                &[
                    "ls-files",
                    "-z",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "--",
                    "crates/",
                    "mobile/ios/",
                ],
            )?;

            for name in names.split('\0') {
                let path = PathBuf::from(name);

                if tracked_language(name).is_some() && path.try_exists()? {
                    collect_files(&path, &mut files)?;
                }
            }
        }
    } else {
        for path in paths {
            collect_files(&path, &mut files)?;
        }
    }

    let mut totals = Totals::default();

    let checked = files.len() + staged.len();

    for (path, language, source, ranges) in staged {
        let inspection = inspect(language, &path, &source, &options)?;

        report(&path, &inspection, Some(&ranges), &mut totals);
    }

    for path in files {
        let Some(language) = language(&path) else {
            continue;
        };

        let source = fs::read_to_string(&path)?;

        let mut inspection = inspect(language, &path, &source, &options)?;

        if mode == "--fix" && !inspection.spacing.is_empty() {
            let modified = match language {
                Language::Rust => spacing::apply(&source, &inspection.spacing)?,
                Language::Swift => swift::apply(&source, &inspection.spacing)?,
            };

            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));

            let mut temporary = NamedTempFile::new_in(parent)?;

            temporary.write_all(modified.as_bytes())?;

            temporary
                .as_file()
                .set_permissions(fs::metadata(&path)?.permissions())?;

            if fs::read(&path)? != source.as_bytes() {
                return Err(format!(
                    "{}: changed while checking; file left untouched",
                    path.display()
                )
                .into());
            }

            temporary.persist(&path)?;

            println!(
                "{}: fixed {} spacing issue(s)",
                path.display(),
                inspection.spacing.len()
            );

            totals.spacing += inspection.spacing.len();

            inspection = Inspection {
                spacing: Issues::new(),
                ..inspect(language, &path, &modified, &options)?
            };
        }

        report(&path, &inspection, None, &mut totals);
    }

    println!(
        "readability: checked {checked} source file(s), {} spacing issue(s){}, {} declaration issue(s), {} expression issue(s)",
        totals.spacing,
        if mode == "--fix" { " fixed" } else { "" },
        totals.declarations,
        totals.expressions
    );

    if (totals.spacing > 0 || totals.declarations > 0 || totals.expressions > 0)
        && mode == "--staged"
    {
        eprintln!(
            "readability: run cargo readability --fix <path> for spacing, correct declaration and expression issues, review the diff, then stage the intended changes"
        );
    }

    Ok((totals.spacing == 0 || mode == "--fix")
        && totals.declarations == 0
        && totals.expressions == 0)
}

fn inspect(
    language: Language,
    path: &Path,
    source: &str,
    options: &Options,
) -> Result<Inspection, Box<dyn Error>> {
    match language {
        Language::Rust => {
            let parsed = parse(path, source)?;

            let declarations = declarations::inspect(&parsed.items)
                .into_iter()
                .map(|issue| Finding {
                    category: Category::Declarations,
                    rule: issue.rule,
                    message: issue.message,
                    start: issue.span.start(),
                    lines: [issue.span, issue.related]
                        .iter()
                        .map(|span| (span.start().line, span.end().line))
                        .collect(),
                });

            let expressions = expressions::inspect(&parsed, &options.expressions)
                .into_iter()
                .map(|issue| Finding {
                    category: Category::Expressions,
                    rule: issue.rule,
                    message: issue.message,
                    start: issue.span.start(),
                    lines: vec![(issue.span.start().line, issue.span.end().line)],
                });

            Ok(Inspection {
                spacing: spacing::inspect(source, &parsed, &options.spacing),
                findings: declarations.chain(expressions).collect(),
                unrecognized: None,
            })
        }
        Language::Swift => swift::inspect(source, options),
    }
}

/// Prints the diagnostics of one file. With `ranges`, only diagnostics next to
/// a staged change are printed, so older issues elsewhere in the file are left
/// for an explicit full check.
fn report(
    path: &Path,
    inspection: &Inspection,
    ranges: Option<&[RangeInclusive<usize>]>,
    totals: &mut Totals,
) {
    if let Some(position) = inspection.unrecognized {
        eprintln!(
            "readability: {}:{}:{}: unrecognized Swift syntax; checks skip the affected code",
            path.display(),
            position.line,
            position.column + 1
        );
    }

    for (line, issue) in &inspection.spacing {
        if ranges.is_none_or(|ranges| {
            ranges
                .iter()
                .any(|range| *range.start() <= issue.through_line && *range.end() >= *line)
        }) {
            println!(
                "{}:{}:1: spacing/{}: {}",
                path.display(),
                line + 1,
                issue.rule,
                issue.message()
            );

            totals.spacing += 1;
        }
    }

    for finding in &inspection.findings {
        if ranges.is_none_or(|ranges| {
            finding.lines.iter().any(|(start, end)| {
                ranges
                    .iter()
                    .any(|range| *range.start() < *end && *range.end() >= start - 1)
            })
        }) {
            let category = match finding.category {
                Category::Declarations => {
                    totals.declarations += 1;

                    "declarations"
                }
                Category::Expressions => {
                    totals.expressions += 1;

                    "expressions"
                }
            };

            println!(
                "{}:{}:{}: {category}/{}: {}",
                path.display(),
                finding.start.line,
                finding.start.column + 1,
                finding.rule,
                finding.message
            );
        }
    }
}

fn language(path: &Path) -> Option<Language> {
    match path.extension()?.to_str()? {
        "rs" => Some(Language::Rust),
        "swift" => Some(Language::Swift),
        _ => None,
    }
}

/// Repository scans cover Rust sources under crates/ and the iOS app's Swift
/// sources. Other directories hold vendored code and tools with their own
/// conventions.
fn tracked_language(name: &str) -> Option<Language> {
    match language(Path::new(name))? {
        Language::Rust if name.starts_with("crates/") => Some(Language::Rust),
        Language::Swift if name.starts_with("mobile/ios/") => Some(Language::Swift),
        _ => None,
    }
}

fn parse(path: &Path, source: &str) -> Result<syn::File, Box<dyn Error>> {
    syn::parse_file(source).map_err(|error| {
        format!(
            "{}:{}:{}: syntax: {error}",
            path.display(),
            error.span().start().line,
            error.span().start().column + 1
        )
        .into()
    })
}

fn git(directory: &Path, arguments: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new("git")
        .current_dir(directory)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .args(arguments)
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            arguments[0],
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }

    Ok(String::from_utf8(output.stdout)?)
}

fn changed_boundaries(diff: &str) -> Result<Vec<RangeInclusive<usize>>, Box<dyn Error>> {
    let mut ranges = Vec::new();

    for line in diff.lines().filter(|line| line.starts_with("@@ ")) {
        let location = line
            .split_whitespace()
            .nth(2)
            .and_then(|part| part.strip_prefix('+'))
            .ok_or("invalid Git hunk header")?;

        let (start, count) = location.split_once(',').unwrap_or((location, "1"));
        let start: usize = start.parse()?;
        let count: usize = count.parse()?;

        // A deletion can remove the only blank line without adding any text.
        // Include both edges of additions and the join left by a deletion.
        ranges.push(if count == 0 {
            start..=start
        } else {
            start.saturating_sub(1)..=start + count - 1
        });
    }

    Ok(ranges)
}

fn collect_files(path: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), Box<dyn Error>> {
    let metadata = fs::symlink_metadata(path)?;

    if metadata.file_type().is_symlink() {
        return Err(format!("{}: symbolic links are not supported", path.display()).into());
    }

    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;

            if entry.file_name() != "target" && entry.file_name() != ".git" {
                collect_files(&entry.path(), files)?;
            }
        }
    } else if language(path).is_some() {
        files.insert(path.to_path_buf());
    }

    Ok(())
}
