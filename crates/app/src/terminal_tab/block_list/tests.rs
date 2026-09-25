use std::collections;
use std::ops::Range;
use std::sync::Arc;

use nmt_config::colors::term::TermColors;
use nmt_terminal::ghostty::{
    BlockHandle, BlockRef, GhosttyTerminal, Palette, RowCell, ScreenRowRead,
};
use nmt_terminal::session::BlockPoint as FrozenPoint;
use nmt_terminal::session::page::{PageSource, RowPage};

use crate::terminal_tab::block_list::FrozenView;
use crate::terminal_tab::block_list::geometry::{ITEM_PAD_ROWS, visible_rows};
use crate::terminal_tab::block_list::images::frozen_block_images;
use crate::terminal_tab::block_list::rows::{
    HandleItemInfo, frozen_block_view as frozen_page_view,
};
use crate::terminal_tab::block_list::selection::BlockListPoint;
use crate::terminal_tab::frame::BackgroundColors;
use crate::terminal_tab::pane_model::FrameTheme;
use crate::terminal_tab::pane_model::frozen_hit_map::FrozenHitInfo;
use crate::terminal_tab::theme;

fn row_texts(view: &FrozenView) -> Vec<String> {
    view.rows
        .iter()
        .map(|r| {
            r.line
                .text()
                .replace('\u{00a0}', " ")
                .trim_end()
                .to_string()
        })
        .collect()
}

fn finished_block(
    vt: &[u8],
    cols: u16,
    rows: u16,
) -> (GhosttyTerminal, BlockHandle, HandleItemInfo) {
    let mut t = GhosttyTerminal::new(cols, rows, 10_000).unwrap();

    t.write_vt(vt);

    let handle = t.finish_block().unwrap().expect("block created");
    let rows = t.block_row_count(handle).unwrap();

    let info = HandleItemInfo {
        rows,
        accent: theme::BLOCK_SUCCESS_COLOR,
        header: Some("cmd · ✓".into()),
    };

    (t, handle, info)
}

/// Only the requested row range materializes; skipped head rows keep
/// their item-local y so geometry never shifts.
#[test]
fn frozen_block_view_windows_visible_rows() {
    let (t, handle, info) = finished_block(b"r0\r\nr1\r\nr2\r\n", 10, 5);

    assert_eq!(info.rows, 3);

    let (block, palette) = (t.block_acquire(handle).expect("acquire"), t.color_palette());

    let view = frozen_block_view(
        Some((&block, &palette)),
        &info,
        0,
        1..2,
        10.0,
        ITEM_PAD_ROWS,
        None,
        &default_colors(),
    );

    assert_eq!(row_texts(&view), ["r1"]);
    assert_eq!(view.rows[0].y, 20.0, "pad + one skipped row");
    assert_eq!(view.active_top, 50.0, "full item height regardless");
}

/// Selection spans map straight onto physical rows.
#[test]
fn frozen_block_view_selection_spans_rows() {
    let (t, handle, info) = finished_block(b"aaaa\r\nbbbb\r\ncccc\r\n", 10, 5);

    let (block, palette) = (t.block_acquire(handle).expect("acquire"), t.color_palette());

    let sel = Some((
        FrozenPoint {
            item: 0,
            line: 0,
            col: 2,
        },
        FrozenPoint {
            item: 0,
            line: 2,
            col: 1,
        },
    ));

    let view = frozen_block_view(
        Some((&block, &palette)),
        &info,
        0,
        0..info.rows,
        10.0,
        ITEM_PAD_ROWS,
        sel,
        &default_colors(),
    );

    let spans: Vec<Option<(u16, u16)>> = view.rows.iter().map(|r| r.selected).collect();

    assert_eq!(
        spans,
        [Some((2, 10)), Some((0, 10)), Some((0, 2))],
        "endpoint rows partial, middle row full width"
    );
}

#[test]
fn frozen_selection_expands_wide_character() {
    let (t, handle, info) = finished_block("中A".as_bytes(), 10, 2);

    let (block, palette) = (t.block_acquire(handle).expect("acquire"), t.color_palette());

    for col in [0, 1] {
        let point = FrozenPoint {
            item: 0,
            line: 0,
            col,
        };

        let view = frozen_block_view(
            Some((&block, &palette)),
            &info,
            0,
            0..info.rows,
            10.0,
            ITEM_PAD_ROWS,
            Some((point, point)),
            &default_colors(),
        );

        assert_eq!(view.rows[0].selected, Some((0, 2)));
    }
}

/// The visible-row window clamps to the item and pads with overdraw.
#[test]
fn visible_rows_clamps_to_item() {
    // Item fully above the viewport (scrolled past): empty range.
    assert_eq!(
        visible_rows(-10_000.0, 50, 600.0, 10.0, ITEM_PAD_ROWS),
        50..50
    );

    // Item starting far below the viewport bottom: empty range.
    assert_eq!(visible_rows(10_000.0, 50, 600.0, 10.0, ITEM_PAD_ROWS), 0..0);

    // Item spanning the viewport: rows around the visible band only.
    let range = visible_rows(-1000.0, 1000, 600.0, 10.0, ITEM_PAD_ROWS);

    assert!(range.start > 0 && range.end < 1000);
    assert!(range.contains(&100), "row at viewport top included");
}

/// The hit map preserves whether a row belongs to a frozen block or the
/// live grid's absolute SCREEN history.
#[test]
fn hit_test_maps_block_list_points() {
    let mut hit = FrozenHitInfo::default();

    hit.push_row(10.0, 0, 0, 10); // item 0 row 0 at y=10

    hit.push_row(20.0, 0, 1, 10);

    hit.push_row(50.0, usize::MAX, 0, 10); // live-history sentinel

    hit.set_active_top(70.0);

    assert_eq!(
        hit.hit_test(35.0, 15.0, 10.0, 10.0, 10, ITEM_PAD_ROWS),
        Some(BlockListPoint::Frozen(FrozenPoint {
            item: 0,
            line: 0,
            col: 3
        }))
    );
    assert_eq!(
        hit.hit_test(15.0, 25.0, 10.0, 10.0, 10, ITEM_PAD_ROWS),
        Some(BlockListPoint::Frozen(FrozenPoint {
            item: 0,
            line: 1,
            col: 1
        }))
    );

    // Beyond the row width clamps to the last column.
    assert_eq!(
        hit.hit_test(500.0, 15.0, 10.0, 10.0, 10, ITEM_PAD_ROWS),
        Some(BlockListPoint::Frozen(FrozenPoint {
            item: 0,
            line: 0,
            col: 9
        }))
    );
    assert_eq!(
        hit.hit_test(0.0, 55.0, 10.0, 10.0, 10, ITEM_PAD_ROWS),
        Some(BlockListPoint::LiveHistory { row: 0, col: 0 }),
        "live history keeps its SCREEN row"
    );
    assert_eq!(
        hit.hit_test(0.0, 5.0, 10.0, 10.0, 10, ITEM_PAD_ROWS),
        None,
        "above rows"
    );
}

/// Frozen Kitty direct read: a placement frozen into a
/// block reports a block-relative row, its pixels read back lazily, and
/// the paint mapping lands on the right visible row band.
#[test]
fn frozen_block_images_map_visible_rows() {
    use crate::terminal_tab::graphics::{ReleaseQueue, graphic_to_generation};

    let mut t = GhosttyTerminal::new(20, 5, 10_000).unwrap();

    t.resize(20, 5, 10, 20).unwrap(); // cell pixel size for grid math

    t.write_vt(b"a\r\nb\r\n");

    t.write_vt(b"\x1b_Ga=T,f=32,s=1,v=1,i=1;/wAA/w==\x1b\\");

    let handle = t.finish_block().unwrap().expect("block created");
    let block = t.block_acquire(handle).expect("acquire");

    let placements = t.block_placements(&block);

    assert_eq!(placements.len(), 1, "one frozen placement");

    let p = placements[0];

    assert_eq!(p.image_id, 1);
    assert_eq!((p.screen_col, p.screen_row), (0, 2), "block-relative row");
    assert!(p.grid_cols >= 1 && p.grid_rows >= 1);

    let data = t.block_image_pixels(&block, 1).expect("frozen pixels");

    assert_eq!((data.width, data.height), (1, 1));
    assert!(t.block_image_pixels(&block, 999).is_none(), "unknown id");

    let q: ReleaseQueue = Default::default();
    let generation = graphic_to_generation(data, &q).unwrap();

    let mut generations = collections::HashMap::new();

    generations.insert(1u32, generation);

    let images = frozen_block_images(&placements, &generations, &(0..3), 10.0, ITEM_PAD_ROWS);

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].y, 10.0 + 2.0 * 10.0, "pad + block row 2");
    assert_eq!((images[0].col, images[0].width), (0, p.grid_cols));

    // Rows outside the visible window materialize nothing.
    assert!(
        frozen_block_images(&placements, &generations, &(0..2), 10.0, ITEM_PAD_ROWS).is_empty()
    );

    // A missing generation is skipped (retry next frame), not painted.
    assert!(
        frozen_block_images(
            &placements,
            &Default::default(),
            &(0..3),
            10.0,
            ITEM_PAD_ROWS
        )
        .is_empty()
    );
}

/// Kitty V1 per-block ownership intentionally differs from active-screen ownership:
/// cross-block place-by-id falls flat on the fresh
/// screen, and an active delete-all cannot reach a frozen block's images.
#[test]
fn kitty_v1_per_block_ownership_deviations() {
    let mut t = GhosttyTerminal::new(20, 5, 10_000).unwrap();

    t.resize(20, 5, 10, 20).unwrap();

    t.write_vt(b"\x1b_Ga=T,f=32,s=1,v=1,i=7;/wAA/w==\x1b\\");

    let frozen = t.finish_block().unwrap().expect("block created");

    // Cross-block place-by-id: the new screen's storage is empty, so a
    // A placement-only command references nothing; a future implementation could
    // forward the image definition table if this pattern matters).
    t.write_vt(b"\x1b_Ga=p,i=7\x1b\\");

    assert!(!t.kitty_image_exists(7), "new screen storage starts empty");

    // Active delete-all only touches active storage so frozen blocks remain immutable:
    // frozen block keeps showing its freeze-time pixels.
    t.write_vt(b"\x1b_Ga=d\x1b\\");

    let block = t.block_acquire(frozen).expect("acquire");

    assert!(
        t.block_image_pixels(&block, 7).is_some(),
        "frozen pixels survive an active delete-all"
    );
    assert_eq!(t.block_placements(&block).len(), 1);
}

#[allow(clippy::too_many_arguments)]
fn frozen_block_view(
    block: Option<(&BlockRef, &Palette)>,
    info: &HandleItemInfo,
    item: usize,
    visible: Range<usize>,
    cell_h: f32,
    pad: f32,
    selection: Option<(FrozenPoint, FrozenPoint)>,
    colors: &BackgroundColors,
) -> FrozenView {
    let pages: Vec<_> = block
        .map(|(block, palette)| {
            let handle = block.handle();

            let rows = (0..block.row_count())
                .map(|row| {
                    let mut cells = Vec::new();

                    let meta = block
                        .read_row_visit(row, palette, |x, text, wide, style| {
                            cells.push(RowCell {
                                x,
                                text,
                                wide,
                                style,
                            })
                        })
                        .unwrap()
                        .unwrap();

                    ScreenRowRead { cells, meta }
                })
                .collect();

            Arc::new(RowPage {
                source: PageSource::Block {
                    id: handle.id,
                    generation: handle.generation,
                    theme: 0,
                },
                start: 0,
                cols: block.cols(),
                rows,
                placements: Vec::new(),
            })
        })
        .into_iter()
        .collect();

    frozen_page_view(&pages, info, item, visible, cell_h, pad, selection, colors)
}

fn default_colors() -> BackgroundColors {
    BackgroundColors::new(TermColors::default(), &FrameTheme::default())
}
