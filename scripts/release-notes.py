#!/usr/bin/env python3
"""Render the changelog of a release tag from its commit subjects.

The range starts at the previous release tag reachable from the tag's parent,
so a tag cut from a branch that skipped a release still lists everything since
the release it was built on. Only `feat`, `fix` and `perf` subjects are kept:
the remaining Conventional Commit types describe work nobody running the build
can observe.

The same commits render as Markdown for the GitHub release body and the Windows
updater, and as HTML for the Sparkle appcast, whose update dialog renders an
item's description as HTML and has no Markdown support.
"""

import argparse
import html
import re
import subprocess

SECTIONS = [
    ("feat", "Features"),
    ("fix", "Fixes"),
    ("perf", "Performance"),
]

SUBJECT = re.compile(r"^(?P<type>[a-z]+)(?:\((?P<scope>[^)]+)\))?!?: (?P<text>.+)$")


def git(*args):
    return subprocess.run(
        ["git", *args], check=True, capture_output=True, text=True, encoding="utf-8"
    ).stdout


def previous_tag(tag, match):
    """The newest tag matching `match` behind `tag`, or None for the first one."""
    try:
        return git("describe", "--tags", "--abbrev=0", "--match", match, f"{tag}^").strip()
    except subprocess.CalledProcessError:
        return None


def collect(tag, match):
    """Kept entries grouped by type, oldest first, as (scope, text) pairs."""
    previous = previous_tag(tag, match)
    revisions = f"{previous}..{tag}" if previous else tag
    subjects = git("log", "--no-merges", "--reverse", "--format=%s", revisions).splitlines()

    groups = {kind: [] for kind, _ in SECTIONS}
    for subject in subjects:
        parsed = SUBJECT.match(subject.strip())
        if parsed and parsed["type"] in groups:
            groups[parsed["type"]].append((parsed["scope"], parsed["text"]))
    return groups


def markdown(groups):
    blocks = []
    for kind, title in SECTIONS:
        if not groups[kind]:
            continue
        lines = [f"### {title}", ""]
        for scope, text in groups[kind]:
            lines.append(f"- **{scope}**: {text}" if scope else f"- {text}")
        blocks.append("\n".join(lines))
    return "\n\n".join(blocks)


def to_html(groups):
    blocks = []
    for kind, title in SECTIONS:
        if not groups[kind]:
            continue
        items = []
        for scope, text in groups[kind]:
            text = html.escape(text)
            items.append(
                f"<li><b>{html.escape(scope)}</b>: {text}</li>" if scope else f"<li>{text}</li>"
            )
        blocks.append(f"<h3>{title}</h3>\n<ul>\n" + "\n".join(items) + "\n</ul>")
    return "\n".join(blocks)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True, help="release tag, for example v1.7.0")
    parser.add_argument(
        "--match", default="v[0-9]*", help="glob naming the tags a range starts from"
    )
    parser.add_argument("--format", choices=["markdown", "html"], default="markdown")
    args = parser.parse_args()

    groups = collect(args.tag, args.match)
    print(markdown(groups) if args.format == "markdown" else to_html(groups))


if __name__ == "__main__":
    main()
