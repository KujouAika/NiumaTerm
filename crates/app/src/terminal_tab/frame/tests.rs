use nmt_config::colors::NamedColor;
use nmt_terminal::ghostty::GhosttyTerminal;
use nmt_terminal::grid::{Column, Line, Pos};
use nmt_terminal::render_buffer::RenderBuffer;
use nmt_terminal::selection::SelectionRange;

use crate::terminal_tab::frame::{
    BackgroundColors, EngineRowBuilder, FrameImageKind, GenerationMap, TerminalColor,
    TerminalFrame, TerminalLine, ZLayer, extract_frame_images, extract_row,
    extract_row_with_colors, line_from_parts,
};
// --- Kitty image frame extraction ---
use crate::terminal_tab::graphics;
use crate::terminal_tab::graphics::graphic_to_generation;
use crate::terminal_tab::layout::frame_content_rows;
use crate::terminal_tab::pane_model::FrameTheme;
use crate::terminal_tab::pane_model::frame_cache::TerminalFrameCache;

fn frame_with_line(line: &str) -> TerminalFrame {
    TerminalFrame {
        lines: [line_from_parts(line.to_owned(), Vec::new(), Vec::new())].into(),
        line_states: [Default::default()].into(),
        cols: line.len(),
        cursor: None,
        layout_cursor_row: None,
        scrollbar: Default::default(),
        images: [].into(),
    }
}

fn first_line(frame: &TerminalFrame) -> &str {
    frame.lines()[0].text().as_ref()
}

#[test]
fn application_hidden_and_offscreen_cursors_do_not_extend_content() {
    let mut engine = GhosttyTerminal::new(20, 3, 100).unwrap();

    engine.write_vt(b"output\x1b[3;1H\x1b[?25l");

    let frame = TerminalFrame::from_render_buffer(&engine.snapshot().unwrap());

    assert!(frame.cursor().is_none());
    assert_eq!(frame.layout_cursor_row(), None);
    assert_eq!(frame_content_rows(&frame), 1);

    engine.write_vt(b"\x1b[2J\x1b[H\x1b[?25h\r\n\r\n\r\n\r\n\r\nPrompt>");
    engine.scroll_viewport_top();

    let frame = TerminalFrame::from_render_buffer(&engine.snapshot().unwrap());

    assert!(frame.cursor().is_none());
    assert_eq!(frame.layout_cursor_row(), None);
    assert_eq!(frame_content_rows(&frame), 0);
}

#[test]
fn extracted_rows_have_stable_content_hashes() {
    let mut engine = GhosttyTerminal::new(4, 1, 100).unwrap();

    engine.write_vt(b"ab");

    let mut buf = RenderBuffer::new(4, 1);

    engine.snapshot_into(&mut buf, 0, 0).unwrap();

    let first = extract_row(&buf, 0, None);
    let second = extract_row(&buf, 0, None);

    assert_eq!(first.text_hash(), second.text_hash());

    engine.write_vt(b"c");

    engine.snapshot_into(&mut buf, 0, 0).unwrap();

    let changed = extract_row(&buf, 0, None);

    assert_ne!(first.text_hash(), changed.text_hash());
}

/// Regression (broken-selection bug): invalidation marks the cache for
/// rebuild but keeps serving the last frame, so pointer/IME mapping between
/// a mouse event and the next render still sees the displayed frame instead
/// of an empty offsets table.
#[test]
fn cache_serves_stale_frame_until_rebuilt() {
    let mut cache = TerminalFrameCache::default();

    assert!(cache.needs_rebuild(), "empty cache must rebuild");

    cache.rebuild(frame_with_line("first"));

    assert!(!cache.needs_rebuild());
    assert_eq!(first_line(&cache.current().unwrap()), "first");

    cache.invalidate();

    assert!(cache.needs_rebuild(), "invalidation forces a rebuild");
    assert!(
        cache.reusable_frame().is_some(),
        "a plain invalidation keeps the frame eligible for line reuse"
    );
    assert_eq!(
        first_line(&cache.current().unwrap()),
        "first",
        "stale frame stays available for pointer mapping"
    );

    cache.rebuild(frame_with_line("second"));

    assert!(!cache.needs_rebuild());
    assert_eq!(first_line(&cache.current().unwrap()), "second");
}

#[test]
fn cache_full_invalidation_retains_frame_but_disables_reuse_once() {
    let mut cache = TerminalFrameCache::default();

    cache.rebuild(frame_with_line("first"));

    cache.invalidate_full();

    assert!(cache.needs_rebuild());
    assert_eq!(first_line(&cache.current().unwrap()), "first");
    assert!(cache.reusable_frame().is_none());

    cache.rebuild(frame_with_line("second"));

    assert!(!cache.needs_rebuild());
    assert_eq!(
        first_line(&cache.reusable_frame().expect("reuse restored")),
        "second"
    );
}

#[test]
fn incremental_extraction_reuses_only_clean_rows() {
    let mut engine = GhosttyTerminal::new(8, 3, 100).unwrap();
    let mut buf = RenderBuffer::new(8, 3);

    engine.write_vt(b"\x1b[2;1H");

    engine.snapshot_into(&mut buf, 0, 0).unwrap();

    let generations = GenerationMap::new();

    let first = TerminalFrame::from_render_buffer_reusing(
        &buf,
        None,
        &generations,
        None,
        &FrameTheme::default(),
    );

    engine.snapshot_into(&mut buf, 0, 0).unwrap();

    let clean = TerminalFrame::from_render_buffer_reusing(
        &buf,
        None,
        &generations,
        Some(&first),
        &FrameTheme::default(),
    );

    assert!(
        first
            .lines()
            .iter()
            .zip(clean.lines())
            .all(|(old, new)| old.ptr_eq(new)),
        "clean capture reuses every line"
    );

    engine.write_vt(b"X");

    engine.snapshot_into(&mut buf, 0, 0).unwrap();

    let changed = TerminalFrame::from_render_buffer_reusing(
        &buf,
        None,
        &generations,
        Some(&clean),
        &FrameTheme::default(),
    );

    assert!(clean.lines()[0].ptr_eq(&changed.lines()[0]));
    assert!(!clean.lines()[1].ptr_eq(&changed.lines()[1]));
    assert!(clean.lines()[2].ptr_eq(&changed.lines()[2]));

    let forced = TerminalFrame::from_render_buffer_reusing(
        &buf,
        None,
        &generations,
        None,
        &FrameTheme::default(),
    );

    assert!(
        changed
            .lines()
            .iter()
            .zip(forced.lines())
            .all(|(old, new)| !old.ptr_eq(new)),
        "no reusable frame forces full line extraction"
    );
}

#[test]
fn supplied_theme_controls_selection_without_installed_globals() {
    let mut engine = GhosttyTerminal::new(8, 2, 100).unwrap();

    engine.write_vt(b"theme\x1b[?25l");

    let mut buffer = RenderBuffer::new(8, 2);

    engine.snapshot_into(&mut buffer, 0, 0).unwrap();

    let generations = GenerationMap::new();

    let mut theme = FrameTheme::default();

    let selection = Some(SelectionRange::new(
        Pos::new(Line(0), Column(0)),
        Pos::new(Line(0), Column(4)),
        false,
    ));

    let first =
        TerminalFrame::from_render_buffer_reusing(&buffer, selection, &generations, None, &theme);

    theme.selection_background = (0x12, 0x34, 0x56).into();

    let mut cache = TerminalFrameCache::default();

    cache.rebuild(first.clone());

    cache.invalidate_full();

    let next = TerminalFrame::from_render_buffer_reusing(
        &buffer,
        selection,
        &generations,
        cache.reusable_frame().as_ref(),
        &theme,
    );

    assert_eq!(
        next.lines()[0].cells()[0].background,
        Some(theme.selection_background)
    );
    assert!(!first.lines()[0].ptr_eq(&next.lines()[0]));
}

#[test]
fn cursor_only_change_rebuilds_affected_row() {
    let mut engine = GhosttyTerminal::new(8, 2, 100).unwrap();
    let mut buf = RenderBuffer::new(8, 2);

    engine.write_vt(b"AB");

    engine.snapshot_into(&mut buf, 0, 0).unwrap();

    let generations = GenerationMap::new();

    let first = TerminalFrame::from_render_buffer_reusing(
        &buf,
        None,
        &generations,
        None,
        &FrameTheme::default(),
    );

    let versions = buf.row_versions().to_vec();

    engine.write_vt(b"\r");

    engine.snapshot_into(&mut buf, 0, 0).unwrap();

    assert_eq!(buf.row_versions(), versions, "CR changes only the cursor");

    let moved = TerminalFrame::from_render_buffer_reusing(
        &buf,
        None,
        &generations,
        Some(&first),
        &FrameTheme::default(),
    );

    assert!(!first.lines()[0].ptr_eq(&moved.lines()[0]));
    assert!(first.lines()[1].ptr_eq(&moved.lines()[1]));
}

#[test]
fn selection_changes_rebuild_only_affected_rows() {
    let mut engine = GhosttyTerminal::new(8, 3, 100).unwrap();
    let mut buf = RenderBuffer::new(8, 3);

    engine.write_vt(b"row0\r\nrow1\r\nrow2");

    engine.snapshot_into(&mut buf, 0, 0).unwrap();

    let generations = GenerationMap::new();

    let plain = TerminalFrame::from_render_buffer_reusing(
        &buf,
        None,
        &generations,
        None,
        &FrameTheme::default(),
    );

    let row0 = SelectionRange::new(
        Pos::new(Line(0), Column(0)),
        Pos::new(Line(0), Column(3)),
        false,
    );

    let selected = TerminalFrame::from_render_buffer_reusing(
        &buf,
        Some(row0),
        &generations,
        Some(&plain),
        &FrameTheme::default(),
    );

    assert!(!plain.lines()[0].ptr_eq(&selected.lines()[0]));
    assert!(plain.lines()[1].ptr_eq(&selected.lines()[1]));
    assert!(plain.lines()[2].ptr_eq(&selected.lines()[2]));

    let cleared = TerminalFrame::from_render_buffer_reusing(
        &buf,
        None,
        &generations,
        Some(&selected),
        &FrameTheme::default(),
    );

    assert!(!selected.lines()[0].ptr_eq(&cleared.lines()[0]));
    assert!(selected.lines()[1].ptr_eq(&cleared.lines()[1]));
    assert!(selected.lines()[2].ptr_eq(&cleared.lines()[2]));
}

#[test]
fn wide_char_gets_placeholder_and_runs_cover_text() {
    let mut engine = GhosttyTerminal::new(6, 1, 100).unwrap();

    engine.write_vt("中A".as_bytes());

    let mut buf = RenderBuffer::new(6, 1);

    engine.snapshot_into(&mut buf, 0, 0).unwrap();

    let row = extract_row(&buf, 0, None);

    // The wide glyph is followed by a blank placeholder for its 2nd column.
    assert!(row.text().as_ref().starts_with("中\u{00a0}A"));

    // Force-width layout needs run byte-lengths to sum to the row text length.
    let run_bytes: usize = row.runs().iter().map(|run| run.len).sum();

    assert_eq!(run_bytes, row.text().len());
}

#[test]
fn bold_toggle_changes_shape_cache_key() {
    let mut plain_engine = GhosttyTerminal::new(4, 1, 100).unwrap();

    plain_engine.write_vt(b"A");

    let mut plain_buf = RenderBuffer::new(4, 1);

    plain_engine.snapshot_into(&mut plain_buf, 0, 0).unwrap();

    let plain = extract_row(&plain_buf, 0, None);

    let mut bold_engine = GhosttyTerminal::new(4, 1, 100).unwrap();

    bold_engine.write_vt(b"\x1b[1mA");

    let mut bold_buf = RenderBuffer::new(4, 1);

    bold_engine.snapshot_into(&mut bold_buf, 0, 0).unwrap();

    let bold = extract_row(&bold_buf, 0, None);

    // Same visible text, but bold must not reuse the plain shaped glyphs.
    assert_eq!(plain.text(), bold.text());
    assert_ne!(plain.text_hash(), bold.text_hash());
}

/// Run `vt` through the engine, mirror it into a `RenderBuffer`, and build a live
/// generation map from the shipped image deltas: the same inputs frame extraction
/// sees at runtime.
fn buf_and_generations(cols: u16, rows: u16, vt: &[u8]) -> (RenderBuffer, GenerationMap) {
    let mut engine = GhosttyTerminal::new(cols, rows, 100).unwrap();

    engine.resize(cols, rows, 10, 20).unwrap();

    engine.write_vt(vt);

    let buf = engine.snapshot().unwrap();

    let release: graphics::ReleaseQueue = Default::default();
    let (pending, _) = engine.take_image_deltas(buf.placements());

    let mut generations = GenerationMap::new();

    for (id, data) in pending {
        if let Some(g) = graphic_to_generation(data, &release) {
            generations.insert(id, g);
        }
    }

    (buf, generations)
}

#[test]
fn extracts_overlay_placement_with_source_and_z() {
    let (buf, generations) =
        buf_and_generations(20, 5, b"\x1b_Ga=T,f=32,s=1,v=1,i=1,p=9;/wAA/w==\x1b\\");

    let images = extract_frame_images(&buf, &generations);

    assert_eq!(images.len(), 1, "one overlay image");

    let img = &images[0];

    assert_eq!(img.z_layer(), ZLayer::AboveText, "z=0 paints above text");

    match img.kind {
        FrameImageKind::Overlay {
            viewport_col,
            viewport_row,
            source,
            ..
        } => {
            assert_eq!((viewport_col, viewport_row), (0, 0));
            assert_eq!(source, [0.0, 0.0, 1.0, 1.0], "full-image source");
        }
        _ => panic!("expected overlay"),
    }
}

#[test]
fn skips_placement_whose_image_is_not_cached() {
    // Same buffer, but an empty generation map (pixels not yet delivered).
    let (buf, _) = buf_and_generations(20, 5, b"\x1b_Ga=T,f=32,s=1,v=1,i=1;/wAA/w==\x1b\\");
    let images = extract_frame_images(&buf, &GenerationMap::new());

    assert!(images.is_empty(), "uncached image is skipped, not failed");
}

#[test]
fn plain_rows_are_not_scanned_for_placeholders() {
    // No virtual placeholders anywhere: extraction yields no virtual images and the
    // per-row fast path skips every row (no panic, empty result).
    let (buf, generations) = buf_and_generations(8, 2, b"hello");

    assert!(!buf.row_has_virtual_placeholder(0));
    assert!(extract_frame_images(&buf, &generations).is_empty());
}

#[test]
fn extracts_contiguous_virtual_run() {
    // A 2×1 virtual image (id=7, p=3, c=2 r=1) with two contiguous placeholder
    // cells that inherit column from the first → one run of width 2.
    // Placement id 0 (no `p=`, no underline color) so the run's decoded
    // placement id (from underline) matches the placement metadata.
    let d0 = '\u{0305}';
    let cell0 = format!("\x1b[38;2;0;0;7m{}{}", '\u{10EEEE}', d0); // row=0,col=0
    let cell1 = format!("{}", '\u{10EEEE}'); // inherit row/col

    let mut vt = Vec::new();

    vt.extend_from_slice(b"\x1b_Ga=T,U=1,f=32,s=2,v=1,i=7,c=2,r=1;/wAA//8AAP8=\x1b\\");

    vt.extend_from_slice(cell0.as_bytes());

    vt.extend_from_slice(cell1.as_bytes());

    let (buf, generations) = buf_and_generations(20, 5, &vt);

    let images = extract_frame_images(&buf, &generations);

    assert_eq!(images.len(), 1, "one virtual run");

    match images[0].kind {
        FrameImageKind::Virtual {
            run,
            placement_cols,
            screen_col,
            screen_line,
            ..
        } => {
            assert_eq!(run.image_id, 7);
            assert_eq!(run.width, 2, "two inherited-column cells form one run");
            assert_eq!(placement_cols, 2);
            assert_eq!((screen_line, screen_col), (0, 0));
        }
        _ => panic!("expected virtual"),
    }
}

#[test]
fn unmatched_placeholder_is_skipped() {
    // Placeholder cells reference image id 9, but no image 9 was transmitted, so
    // there is no matching virtual placement and no cached image → skipped.
    let d0 = '\u{0305}';
    let cell = format!("\x1b[38;2;0;0;9m{}{}{}", '\u{10EEEE}', d0, d0);
    let (buf, generations) = buf_and_generations(20, 5, cell.as_bytes());

    assert!(
        extract_frame_images(&buf, &generations).is_empty(),
        "no matching placement/image → no descriptor, no marker"
    );
}

/// Scrolled-back and frozen rows are built by `EngineRowBuilder` from engine
/// reads; the viewport by `extract_row_with_colors` from the render buffer.
/// The same cells must shape and color identically on both paths: the
/// default foreground under an OSC 10 override, faint text with an explicit
/// and with the default color, inverse video, and a kitty placeholder cell
/// that stays out of the shaped text.
#[test]
fn engine_rows_render_like_viewport_rows() {
    let mut engine = GhosttyTerminal::new(12, 1, 100).unwrap();

    engine.write_vt(b"a \x1b[2m\x1b[38;2;200;100;50mdim\x1b[0m \x1b[7minv\x1b[0m \x1b[2mff");

    let mut buf = RenderBuffer::new(12, 1);

    engine.snapshot_into(&mut buf, 0, 0).unwrap();

    // The runtime default-color layer that OSC 10 writes into.
    let mut term_colors = buf.colors();

    term_colors[NamedColor::Foreground] = Some([0.2, 0.4, 0.6, 1.0]);

    let colors = BackgroundColors::new(term_colors, &FrameTheme::default());
    let viewport = extract_row_with_colors(&buf, 0, None, &colors, None);
    let history = engine_row(&engine, 0, &colors);

    assert_eq!(viewport.runs()[0].fg, (0x33, 0x66, 0x99).into());
    assert_eq!(history.text(), viewport.text());
    assert_eq!(history.runs(), viewport.runs());
    assert_eq!(
        cell_backgrounds(&history),
        cell_backgrounds(&viewport),
        "inverse video paints the same background on both paths"
    );

    let placeholder = format!("\x1b[38;2;0;0;7m{}\u{0305}\u{0305}abc", '\u{10EEEE}');

    let (buf, _) = buf_and_generations(
        4,
        1,
        format!("\x1b_Ga=T,U=1,f=32,s=1,v=1,i=7,p=3,c=1,r=1;/wAA/w==\x1b\\{placeholder}")
            .as_bytes(),
    );

    let mut engine = GhosttyTerminal::new(4, 1, 100).unwrap();

    engine.resize(4, 1, 10, 20).unwrap();
    engine.write_vt(b"\x1b_Ga=T,U=1,f=32,s=1,v=1,i=7,p=3,c=1,r=1;/wAA/w==\x1b\\");
    engine.write_vt(placeholder.as_bytes());

    let colors = BackgroundColors::new(buf.colors(), &FrameTheme::default());
    let viewport = extract_row_with_colors(&buf, 0, None, &colors, None);
    let history = engine_row(&engine, 0, &colors);

    assert!(!viewport.text().contains('\u{10EEEE}'));
    assert_eq!(history.text(), viewport.text());
}

fn engine_row(engine: &GhosttyTerminal, row: u32, colors: &BackgroundColors) -> TerminalLine {
    let data = engine
        .read_screen_row(row, &engine.color_palette())
        .unwrap()
        .unwrap();

    let mut builder = EngineRowBuilder::default();

    for cell in &data.cells {
        builder.push(cell.x, cell.text.clone(), cell.wide, &cell.style, colors);
    }

    builder.into()
}

fn cell_backgrounds(line: &TerminalLine) -> Vec<(u16, Option<TerminalColor>)> {
    line.cells()
        .iter()
        .filter(|cell| cell.ch != ' ' && cell.ch != '\0')
        .map(|cell| (cell.col, cell.background))
        .collect()
}

#[test]
fn placeholder_codepoint_is_suppressed_from_text() {
    let d0 = '\u{0305}';

    let mut vt = Vec::new();

    vt.extend_from_slice(b"\x1b_Ga=T,U=1,f=32,s=1,v=1,i=7,p=3,c=1,r=1;/wAA/w==\x1b\\");

    vt.extend_from_slice(format!("\x1b[38;2;0;0;7m{}{}{}", '\u{10EEEE}', d0, d0).as_bytes());

    let (buf, generations) = buf_and_generations(20, 5, &vt);
    let frame = TerminalFrame::from_render_buffer_with_selection(&buf, None, &generations);

    // The placeholder glyph never reaches shaped text (no U+10EEEE), but the cell
    // still occupies its column (blank).
    assert!(
        !frame.lines()[0].text().as_ref().contains('\u{10EEEE}'),
        "placeholder codepoint suppressed"
    );
}
