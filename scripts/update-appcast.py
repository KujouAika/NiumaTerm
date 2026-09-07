#!/usr/bin/env python3
"""Prepend one release to the Sparkle appcast and keep the document bounded.

Sparkle reads a static RSS feed: one `<item>` per published build, ordered by
`<sparkle:version>`. Nothing on the server decides anything — the enclosure's
EdDSA signature is what makes a download trustworthy — so this only has to
produce well-formed XML carrying the values the packaging job measured.

Rebuilding a revision replaces its existing item instead of adding a second
one, which is what lets the nightly job be rerun on a revision it already
published.
"""

import argparse
import email.utils
import os
import xml.etree.ElementTree as ET
from datetime import datetime, timezone

SPARKLE = "http://www.andymatuschak.org/xml-namespaces/sparkle"
ET.register_namespace("sparkle", SPARKLE)

# Enough history that someone who has been offline for a week still finds a
# path forward, without growing a document every installation downloads on a
# six-hour schedule.
KEEP_PER_CHANNEL = 10

SKELETON = f"""<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="{SPARKLE}">
  <channel>
    <title>NiumaTerm</title>
    <link>https://github.com/f32y/NiumaTerm</link>
    <description>NiumaTerm updates</description>
  </channel>
</rss>
"""


def sparkle_tag(name):
    return f"{{{SPARKLE}}}{name}"


def item_channel(item):
    """The item's channel, or None for the default one every updater sees."""
    node = item.find(sparkle_tag("channel"))
    return node.text if node is not None else None


def build_item(args):
    # A release tag is written v1.2.9 while the bundle reports 1.2.9 in
    # CFBundleShortVersionString. The two have to agree, or Sparkle's dialog
    # shows the user "v1.2.9" replacing "1.2.9".
    short = args.label.removeprefix("v")

    item = ET.Element("item")
    ET.SubElement(item, "title").text = args.label
    ET.SubElement(item, "link").text = args.page_url
    ET.SubElement(item, "pubDate").text = email.utils.format_datetime(
        datetime.now(timezone.utc)
    )
    # An item with no channel element is on the default channel, which every
    # updater can see whatever it asked for.
    if args.channel:
        ET.SubElement(item, sparkle_tag("channel")).text = args.channel
    ET.SubElement(item, sparkle_tag("version")).text = args.bundle_version
    ET.SubElement(item, sparkle_tag("shortVersionString")).text = short
    ET.SubElement(item, sparkle_tag("minimumSystemVersion")).text = args.minimum_system
    ET.SubElement(
        item,
        "enclosure",
        {
            "url": args.url,
            "type": "application/octet-stream",
            "length": args.length,
            sparkle_tag("edSignature"): args.signature,
        },
    )
    return item


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--appcast", required=True, help="feed to rewrite in place")
    parser.add_argument("--label", required=True, help="v1.2.9, or nightly-…")
    parser.add_argument(
        "--bundle-version", required=True, help="the CFBundleVersion Sparkle orders by"
    )
    parser.add_argument("--channel", default="", help="empty for stable")
    parser.add_argument("--url", required=True, help="download URL of the archive")
    parser.add_argument("--page-url", required=True, help="release page URL")
    parser.add_argument("--length", required=True, help="archive size in bytes")
    parser.add_argument("--signature", required=True, help="EdDSA signature")
    parser.add_argument("--minimum-system", default="13.0")
    return parser.parse_args()


def main():
    args = parse_args()

    if os.path.exists(args.appcast):
        tree = ET.parse(args.appcast)
    else:
        tree = ET.ElementTree(ET.fromstring(SKELETON))
    channel = tree.getroot().find("channel")

    for existing in channel.findall("item"):
        version = existing.find(sparkle_tag("version"))
        if version is not None and version.text == args.bundle_version:
            channel.remove(existing)

    # Detached and re-appended in order, because ElementTree has no way to
    # reorder children in place.
    items = channel.findall("item")
    for existing in items:
        channel.remove(existing)
    items.append(build_item(args))
    items.sort(key=lambda item: int(item.find(sparkle_tag("version")).text), reverse=True)

    kept = {}
    for item in items:
        name = item_channel(item)
        kept.setdefault(name, 0)
        if kept[name] < KEEP_PER_CHANNEL:
            kept[name] += 1
            channel.append(item)

    ET.indent(tree, space="  ")
    tree.write(args.appcast, encoding="utf-8", xml_declaration=True)


if __name__ == "__main__":
    main()
