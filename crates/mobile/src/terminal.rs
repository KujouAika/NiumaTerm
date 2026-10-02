//! A view of one terminal session on a host.
//!
//! The host streams VT bytes; this runs them through the same libghostty-vt
//! engine the desktop's remote tabs use and hands the app render-ready rows.
//! The host engine next to the real PTY answers terminal queries, so this
//! engine runs with its responses off: a second answer would reach the
//! program as typed input.

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod terminal_tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use nmt_config::CursorShape;
use nmt_config::colors::term::{List, TermColors};
use nmt_config::colors::{AnsiColor, ColorRgb, Colors, NamedColor};
use nmt_config::system::NewlineShortcut;
use nmt_input::keyboard::ModifiersState;
use nmt_platform::{AsyncPty, runtime};
use nmt_remote::NetworkPty;
use nmt_remote::connection::{RemoteHost, Status};
use nmt_terminal::grid::{Style, StyleFlags};
use nmt_terminal::input::{KeyPhase, TerminalKey};
use nmt_terminal::palette::{indexed_color, resolve_color};
use nmt_terminal::render_buffer::RenderBuffer;
use nmt_terminal::selection::SelectionType;
use nmt_terminal::session::interaction::{InputOutcome, TerminalInteraction};
use nmt_terminal::session::{
    HostEvent, SessionChange, SessionObserver, SurfaceCell, SurfaceCellSide, SurfaceMouseButton,
    SurfaceMouseEventKind, SurfaceScreenCell, TerminalSession,
};
use nmt_terminal::termio::SessionOptions;
use parking_lot::Mutex;
use tokio::task::AbortHandle;

use crate::error::CoreError;
use crate::records::ViewEnd;

/// History kept on the phone. The host checkpoint replays all of its
/// history; lines past this bound scroll out of the phone's engine.
const SCROLLBACK_LINES: usize = 10_000;

/// Engine events carry a route id; each view gets its own so events from
/// two open terminals never mix.
static NEXT_ROUTE: AtomicUsize = AtomicUsize::new(1);

/// Told when a terminal view changed. Called on the core's threads.
#[uniffi::export(with_foreign)]
pub trait TerminalObserver: Send + Sync {
    /// The screen or the view's state changed. Further calls wait until the
    /// app reads a frame, so a flood of output costs one call per frame the
    /// app draws instead of one per batch the host sends.
    fn changed(&self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TerminalCursorShape {
    Block,
    Underline,
    Beam,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TerminalUnderline {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TerminalCursor {
    pub col: u16,
    pub row: u16,
    pub shape: TerminalCursorShape,
}

/// Cells of one row that share a style. Colors are `0xRRGGBB`, already
/// resolved against the palette, with inverse and faint applied.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TerminalRun {
    pub col: u16,

    /// Cells the run covers. A run is either all single-width characters,
    /// one per cell, or exactly one double-width character covering two, so
    /// the app can place every glyph on its cell without measuring text.
    pub cells: u16,

    pub text: String,
    pub fg: u32,

    /// `None` leaves the frame's background showing.
    pub bg: Option<u32>,

    pub bold: bool,
    pub italic: bool,
    pub underline: TerminalUnderline,
    pub strikeout: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TerminalLine {
    pub row: u16,

    /// Blank default-background cells are left out; the app clears the row
    /// before drawing these.
    pub runs: Vec<TerminalRun>,
}

/// A selection in viewport rows. Rows may lie outside the viewport when the
/// selection continues into history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TerminalSelection {
    pub start_row: i32,
    pub start_col: u16,
    pub end_row: i32,
    pub end_col: u16,
    pub block: bool,
}

/// What changed since the app last read a frame.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TerminalFrame {
    pub cols: u16,
    pub rows: u16,

    /// Every row is in `lines`; the app redraws everything. Otherwise only
    /// rows whose content changed are, and the rest stay as drawn.
    pub full: bool,

    pub lines: Vec<TerminalLine>,

    pub foreground: u32,
    pub background: u32,
    pub cursor: Option<TerminalCursor>,
    pub selection: Option<TerminalSelection>,

    /// Rows of history above the viewport's top, and the total rows.
    pub scroll_offset: u64,

    pub scroll_total: u64,

    pub title: String,

    /// The program rang the bell since the last frame.
    pub bell: bool,

    /// Text the program put on the clipboard (OSC 52) since the last frame.
    pub clipboard: Option<String>,

    /// The shell ended, or the host is gone for good.
    pub exited: bool,

    pub ended: Option<ViewEnd>,
}

/// A key the terminal encodes itself: a named key (`enter`, `tab`,
/// `backspace`, `escape`, `left`, `up`, `home`, `pageup`, `f1`, ...) or a
/// character with modifiers. Plain text goes through
/// [`TerminalHandle::send_text`] instead.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TerminalKeyInput {
    pub key: String,

    /// The character the key produces, for character keys.
    pub text: Option<String>,

    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub command: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum KeyResult {
    Sent,
    /// The key means nothing to the terminal.
    Ignored,
    /// The key is the paste chord; the app reads its clipboard and pastes.
    Paste,
    /// The key is the copy chord and text is selected; the app copies it.
    Copy,
}

/// A view of one terminal session on a host. Dropping it detaches; the
/// session keeps running on the host.
#[derive(uniffi::Object)]
pub struct TerminalHandle {
    host: Arc<RemoteHost>,
    session_id: String,
    wake: Arc<Wake>,
    view: Mutex<View>,
    tasks: Vec<AbortHandle>,
}

struct View {
    session: TerminalSession,
    interaction: TerminalInteraction,
    palette: List,

    /// What the app already drew, so a frame includes only changed rows.
    drawn: Option<Drawn>,
}

struct Drawn {
    cols: usize,
    rows: usize,
    viewport_top: Option<u32>,
    theme_revision: u64,
    colors: TermColors,
    row_versions: Vec<u64>,
}

/// Coalesces change reports: one call to the app until it reads a frame.
struct Wake {
    observer: Arc<dyn TerminalObserver>,
    pending: AtomicBool,
}

impl Wake {
    fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            self.observer.changed();
        }
    }
}

/// Engine callbacks run on the PTY worker mid-batch; this only marks the
/// view dirty and never touches the handle's lock.
struct Bridge(Arc<Wake>);

impl SessionObserver for Bridge {
    fn changed(&self, _change: SessionChange) {
        self.0.wake();
    }
}

impl TerminalHandle {
    pub(crate) fn attach(
        host: Arc<RemoteHost>,
        pty: NetworkPty,
        cols: u16,
        rows: u16,
        observer: Arc<dyn TerminalObserver>,
    ) -> Result<Arc<Self>, CoreError> {
        let session_id = pty.session().to_owned();

        let wake = Arc::new(Wake {
            observer,
            pending: AtomicBool::new(false),
        });

        let view = View::start(pty, cols, rows, Arc::clone(&wake))?;

        let ended = runtime()
            .spawn(watch_ended(Arc::clone(&host), Arc::clone(&wake)))
            .abort_handle();

        Ok(Arc::new(Self {
            host,
            session_id,
            wake,
            view: Mutex::new(view),
            tasks: vec![ended],
        }))
    }
}

impl View {
    fn start(pty: impl AsyncPty, cols: u16, rows: u16, wake: Arc<Wake>) -> Result<Self, CoreError> {
        let colors = Colors::default();

        let session = TerminalSession::from_pty(
            pty,
            None,
            SessionOptions {
                cols: cols.max(1),
                rows: rows.max(1),
                route_id: NEXT_ROUTE.fetch_add(1, Ordering::Relaxed),
                colors,
                cursor_shape: CursorShape::Block,
                scrollback_lines: SCROLLBACK_LINES,
                // Block mode freezes finished commands out of the screen for
                // the desktop's block list, which the phone does not draw.
                // Without it history stays plain scrollback.
                engine_blocks: false,
                terminal_responses: false,
            },
            Some(Arc::new(Bridge(wake))),
        )
        .map_err(|error| CoreError::Failed {
            message: format!("the terminal could not start: {error}"),
        })?;

        Ok(Self {
            session,
            interaction: TerminalInteraction::default(),
            palette: List::from(&colors),
            drawn: None,
        })
    }

    /// Rows and state that changed since the last frame.
    fn frame(&mut self) -> TerminalFrame {
        let snapshot = self.session.snapshot();

        let drawn = Drawn {
            cols: snapshot.cols(),
            rows: snapshot.rows(),
            viewport_top: snapshot.viewport_top(),
            theme_revision: snapshot.theme_revision(),
            colors: snapshot.colors(),
            row_versions: snapshot.row_versions().to_vec(),
        };

        // Any change besides row content moves or recolors every row.
        let previous = self
            .drawn
            .take()
            .filter(|previous| {
                previous.cols == drawn.cols
                    && previous.rows == drawn.rows
                    && previous.viewport_top == drawn.viewport_top
                    && previous.theme_revision == drawn.theme_revision
                    && previous.colors == drawn.colors
            })
            .map(|previous| previous.row_versions);

        let colors = CellColors {
            palette: &self.palette,
            overrides: drawn.colors,
        };

        let lines = (0..drawn.rows)
            .filter(|&row| {
                previous
                    .as_ref()
                    .is_none_or(|versions| versions.get(row) != drawn.row_versions.get(row))
            })
            .map(|row| line(&snapshot, row, &colors))
            .collect();

        let mut bell = false;
        let mut clipboard = None;

        for event in self.session.poll_events() {
            match event {
                HostEvent::Bell => bell = true,
                HostEvent::Clipboard { text, .. } => clipboard = Some(text),
                _ => {}
            }
        }

        let scroll = snapshot.scrollbar();

        let frame = TerminalFrame {
            cols: drawn.cols as u16,
            rows: drawn.rows as u16,
            full: previous.is_none(),
            lines,
            foreground: rgb(colors.named(NamedColor::Foreground)),
            background: rgb(snapshot
                .window_bg_override()
                .unwrap_or_else(|| colors.named(NamedColor::Background))),
            cursor: cursor(&snapshot),
            selection: self.selection(&snapshot),
            scroll_offset: scroll.offset,
            scroll_total: scroll.total,
            title: snapshot.title().to_owned(),
            bell,
            clipboard,
            exited: self.session.exited(),
            ended: None,
        };

        self.drawn = Some(drawn);

        frame
    }

    fn send_key(&mut self, key: &TerminalKeyInput) -> KeyResult {
        // Alt on a phone is the terminal's Meta key: the shared encoder
        // types a character's text whatever Alt says, so Meta is the xterm
        // convention of ESC before the key as encoded without it.
        let meta = key.alt && key.text.is_some();

        if meta && !self.session.write_input(b"\x1b") {
            return KeyResult::Ignored;
        }

        let mut modifiers = ModifiersState::empty();

        modifiers.set(ModifiersState::SHIFT, key.shift);
        modifiers.set(ModifiersState::CONTROL, key.control);
        modifiers.set(ModifiersState::ALT, key.alt && !meta);
        modifiers.set(ModifiersState::SUPER, key.command);

        // Desktop platforms deliver a Control chord without its character,
        // and the encoder reads a character as text to type: Ctrl-C with
        // "c" attached would type "c" instead of interrupting.
        let key_char = key.text.as_deref().filter(|_| !key.control);

        let event = TerminalKey {
            key: &key.key,
            key_char,
            modifiers,
            function: false,
            phase: KeyPhase::Press,
        };

        let snapshot = self.session.snapshot();

        match self
            .interaction
            .send_key(&self.session, &snapshot, &event, NewlineShortcut::Off)
        {
            InputOutcome::Written => KeyResult::Sent,
            InputOutcome::Ignored => KeyResult::Ignored,
            InputOutcome::PasteRequested => KeyResult::Paste,
            // The app reads the text through `selected_text`, which it
            // needs for its edit menu anyway.
            InputOutcome::CopyPending(_) => KeyResult::Copy,
        }
    }

    fn selection(&self, snapshot: &RenderBuffer) -> Option<TerminalSelection> {
        let range = self.session.selection_range_in(snapshot)?;

        Some(TerminalSelection {
            start_row: range.start.row.0,
            start_col: range.start.col.0 as u16,
            end_row: range.end.row.0,
            end_col: range.end.col.0 as u16,
            block: range.is_block,
        })
    }

    /// A viewport cell as a history-anchored point, the form the selection
    /// is kept in so it stays on its text while output scrolls.
    fn screen_cell(&self, col: u16, row: u16) -> SurfaceScreenCell {
        let top = self.session.snapshot().viewport_top().unwrap_or(0);

        SurfaceScreenCell {
            col,
            row: top + u32::from(row),
        }
    }
}

#[uniffi::export]
impl TerminalHandle {
    pub fn session(&self) -> String {
        self.session_id.clone()
    }

    /// What changed since the last call. The first call after attaching,
    /// and any after a resize or a scroll through history, includes every
    /// row.
    pub fn frame(&self) -> TerminalFrame {
        // Cleared before reading, so a change arriving while this reads
        // reports again instead of being lost.
        self.wake.pending.store(false, Ordering::Release);

        let mut frame = self.view.lock().frame();

        frame.ended = self.ended(frame.exited);

        frame
    }

    /// Redraw everything on the next frame, as after the app dropped what
    /// it drew.
    pub fn redraw(&self) {
        self.view.lock().drawn = None;

        self.wake.wake();
    }

    /// Text from the keyboard or an input method, as typed.
    pub fn send_text(&self, text: String) -> bool {
        self.view.lock().session.commit_text(&text)
    }

    pub fn send_key(&self, key: TerminalKeyInput) -> KeyResult {
        self.view.lock().send_key(&key)
    }

    /// Paste, bracketed when the program asked for it.
    pub fn paste(&self, text: String) -> bool {
        self.view.lock().session.paste_text(&text)
    }

    /// Scroll by `lines` (positive toward history) at a cell. Programs that
    /// track the mouse receive wheel events instead.
    pub fn scroll(&self, col: u16, row: u16, lines: i32) -> bool {
        self.view.lock().session.apply_scroll(
            SurfaceCell { col, row },
            lines,
            ModifiersState::empty(),
        )
    }

    pub fn scroll_to_bottom(&self) -> bool {
        self.view.lock().session.scroll_to_end()
    }

    /// Whether a tap is a click the program receives, which is when it
    /// tracks the mouse.
    pub fn tracks_mouse(&self) -> bool {
        self.view.lock().session.mouse_reporting_active()
    }

    /// Click at a cell. Only a program tracking the mouse receives it.
    pub fn click(&self, col: u16, row: u16) -> bool {
        let view = self.view.lock();

        if !view.session.mouse_reporting_active() {
            return false;
        }

        let cell = SurfaceCell { col, row };

        [SurfaceMouseEventKind::Down, SurfaceMouseEventKind::Up]
            .into_iter()
            .all(|kind| {
                view.session.apply_mouse(
                    cell,
                    SurfaceCellSide::Left,
                    Some(SurfaceMouseButton::Left),
                    kind,
                    ModifiersState::empty(),
                    SelectionType::Simple,
                )
            })
    }

    /// Start a selection at a cell: the word under it when `word` is set.
    pub fn select_start(&self, col: u16, row: u16, word: bool) -> bool {
        let view = self.view.lock();
        let cell = view.screen_cell(col, row);

        let ty = if word {
            SelectionType::Semantic
        } else {
            SelectionType::Simple
        };

        let started = view.session.apply_screen_selection(
            cell,
            SurfaceCellSide::Left,
            SurfaceMouseEventKind::Down,
            ty,
        );

        drop(view);

        self.wake.wake();

        started
    }

    /// Extend the selection to a cell.
    pub fn select_extend(&self, col: u16, row: u16) -> bool {
        let view = self.view.lock();
        let cell = view.screen_cell(col, row);

        let extended = view.session.apply_screen_selection(
            cell,
            SurfaceCellSide::Right,
            SurfaceMouseEventKind::Move,
            SelectionType::Simple,
        );

        drop(view);

        self.wake.wake();

        extended
    }

    pub fn clear_selection(&self) {
        self.view.lock().session.clear_selection();
        self.wake.wake();
    }

    /// The selected text, when anything is selected.
    pub async fn selected_text(&self) -> Option<String> {
        let request = {
            let view = self.view.lock();
            let snapshot = view.session.snapshot();

            view.session.selected_text_in(&snapshot)?
        };

        request.await.ok()?.ok()
    }

    /// Claim the PTY size for this view's grid.
    pub fn resize(&self, cols: u16, rows: u16, width: u16, height: u16) -> bool {
        self.view
            .lock()
            .session
            .resize(cols.max(1), rows.max(1), width, height)
    }

    /// Take the session back after the person at the host took it.
    pub fn take_control(&self) {
        self.host.reconnect(&self.session_id);
    }

    /// End the session on the host. Host tabs refuse; they belong to the
    /// person at the host.
    pub fn terminate(&self) {
        self.host.terminate(&self.session_id);
    }
}

impl TerminalHandle {
    fn ended(&self, exited: bool) -> Option<ViewEnd> {
        if let Some(reason) = self.host.ended(&self.session_id) {
            return Some(reason.into());
        }

        // The stream closes for good both when the shell ends and when the
        // host stops trusting this device; only the latter leaves the host
        // refusing connections.
        (exited && *self.host.status().borrow() == Status::Refused).then_some(ViewEnd::Unreachable)
    }
}

impl Drop for TerminalHandle {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// The host ending or restoring this device's views changes what the
/// screen shows, though nothing on the terminal changed.
async fn watch_ended(host: Arc<RemoteHost>, wake: Arc<Wake>) {
    let mut changes = host.ended_changes();

    while changes.changed().await.is_ok() {
        wake.wake();
    }
}

struct CellColors<'a> {
    palette: &'a List,
    overrides: TermColors,
}

impl CellColors<'_> {
    fn named(&self, named: NamedColor) -> ColorRgb {
        indexed_color(self.palette, &self.overrides, named as usize)
    }

    fn resolve(&self, color: &AnsiColor, flags: StyleFlags, foreground: bool) -> ColorRgb {
        resolve_color(self.palette, &self.overrides, color, flags, foreground)
    }

    /// Text and background colors of a style, with inverse swapping them.
    /// The default background resolves to `None` so it shows the frame's.
    fn cell(&self, style: &Style) -> (ColorRgb, Option<ColorRgb>) {
        let default_bg = matches!(style.bg, AnsiColor::Named(NamedColor::Background));

        if style.flags.contains(StyleFlags::INVERSE) {
            let fg = if default_bg {
                self.named(NamedColor::Background)
            } else {
                self.resolve(&style.bg, style.flags, false)
            };

            (fg, Some(self.resolve(&style.fg, style.flags, true)))
        } else {
            let bg = (!default_bg).then(|| self.resolve(&style.bg, style.flags, false));

            (self.resolve(&style.fg, style.flags, true), bg)
        }
    }
}

fn line(snapshot: &RenderBuffer, row: usize, colors: &CellColors<'_>) -> TerminalLine {
    let mut runs: Vec<TerminalRun> = Vec::new();

    // Whether the last run may take more cells: a double-width character
    // always stands alone.
    let mut open = false;

    for col in 0..snapshot.cols() {
        let cell = snapshot.cell(col, row);

        if cell.is_spacer() {
            continue;
        }

        let style = snapshot.style(cell.style_id());
        let (fg, bg) = colors.cell(&style);
        let flags = style.flags;
        let wide = cell.is_wide();

        let mut text = String::new();

        if flags.contains(StyleFlags::HIDDEN) {
            text.push(' ');
        } else {
            text.push(match cell.c() {
                '\0' => ' ',
                c => c,
            });

            if let Some(extras) = cell.extras_id().and_then(|id| snapshot.extras().get(&id)) {
                text.extend(&extras.zerowidth);
            }
        }

        let run = TerminalRun {
            col: col as u16,
            cells: if wide { 2 } else { 1 },
            text,
            fg: rgb(fg),
            bg: bg.map(rgb),
            bold: flags.contains(StyleFlags::BOLD),
            italic: flags.contains(StyleFlags::ITALIC),
            underline: underline(flags),
            strikeout: flags.contains(StyleFlags::STRIKEOUT),
        };

        match runs.last_mut() {
            Some(last)
                if open && !wide && last.col + last.cells == run.col && same_style(last, &run) =>
            {
                last.cells += 1;

                last.text.push_str(&run.text);
            }
            _ => runs.push(run),
        }

        open = !wide;
    }

    for run in &mut runs {
        trim_blank_tail(run);
    }

    runs.retain(|run| run.cells > 0);

    TerminalLine {
        row: row as u16,
        runs,
    }
}

fn same_style(a: &TerminalRun, b: &TerminalRun) -> bool {
    a.fg == b.fg
        && a.bg == b.bg
        && a.bold == b.bold
        && a.italic == b.italic
        && a.underline == b.underline
        && a.strikeout == b.strikeout
}

/// Drop trailing spaces that draw nothing over a cleared row: the empty
/// rest of most lines. A run that is only such spaces ends up empty.
fn trim_blank_tail(run: &mut TerminalRun) {
    if run.bg.is_some() || run.underline != TerminalUnderline::None || run.strikeout {
        return;
    }

    while run.cells > 0 && run.text.ends_with(' ') {
        run.text.pop();

        run.cells -= 1;
    }
}

fn underline(flags: StyleFlags) -> TerminalUnderline {
    if flags.contains(StyleFlags::UNDERCURL) {
        TerminalUnderline::Curly
    } else if flags.contains(StyleFlags::DOUBLE_UNDERLINE) {
        TerminalUnderline::Double
    } else if flags.contains(StyleFlags::DOTTED_UNDERLINE) {
        TerminalUnderline::Dotted
    } else if flags.contains(StyleFlags::DASHED_UNDERLINE) {
        TerminalUnderline::Dashed
    } else if flags.contains(StyleFlags::UNDERLINE) {
        TerminalUnderline::Single
    } else {
        TerminalUnderline::None
    }
}

fn cursor(snapshot: &RenderBuffer) -> Option<TerminalCursor> {
    if !snapshot.cursor_visible() {
        return None;
    }

    let shape = match snapshot.cursor_shape() {
        CursorShape::Block => TerminalCursorShape::Block,
        CursorShape::Underline => TerminalCursorShape::Underline,
        CursorShape::Beam => TerminalCursorShape::Beam,
        CursorShape::Hidden => return None,
    };

    let position = snapshot.cursor();
    let row = u16::try_from(position.row.0).ok()?;
    let col = u16::try_from(position.col.0).ok()?;

    (usize::from(row) < snapshot.rows()).then_some(TerminalCursor {
        col: col.min(snapshot.cols().saturating_sub(1) as u16),
        row,
        shape,
    })
}

fn rgb(color: ColorRgb) -> u32 {
    (u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)
}
