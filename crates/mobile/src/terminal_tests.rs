use std::io;
use std::iter::repeat_n;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::thread::sleep;
use std::time::{Duration, Instant};

use nmt_input::keyboard::ModifiersState;
use nmt_platform::{AsyncPty, WinsizeBuilder};
use nmt_terminal::session::SurfaceCell;
use parking_lot::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::terminal::{
    KeyResult, TerminalFrame, TerminalKeyInput, TerminalObserver, TerminalRun, View, Wake,
};

/// A PTY whose output the test writes, standing in for the host stream,
/// and which keeps the input the view sends.
struct ScriptedPty {
    output: UnboundedReceiver<Vec<u8>>,
    buffered: Vec<u8>,
    input: Arc<Mutex<Vec<u8>>>,
}

impl AsyncPty for ScriptedPty {
    fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        while self.buffered.is_empty() {
            match self.output.poll_recv(cx) {
                Poll::Ready(Some(bytes)) => self.buffered = bytes,
                Poll::Ready(None) => return Poll::Ready(Ok(0)),
                Poll::Pending => return Poll::Pending,
            }
        }

        let len = buf.len().min(self.buffered.len());

        buf[..len].copy_from_slice(&self.buffered[..len]);
        self.buffered.drain(..len);

        Poll::Ready(Ok(len))
    }

    fn poll_write(&mut self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.input.lock().extend_from_slice(buf);

        Poll::Ready(Ok(buf.len()))
    }

    fn poll_exit(&mut self, _cx: &mut Context<'_>) -> Poll<()> {
        Poll::Pending
    }

    fn poll_resize(
        &mut self,
        _cx: &mut Context<'_>,
        _size: WinsizeBuilder,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[derive(Default)]
struct Counter(AtomicUsize);

impl TerminalObserver for Counter {
    fn changed(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

struct Started {
    view: View,
    output: UnboundedSender<Vec<u8>>,
    input: Arc<Mutex<Vec<u8>>>,
    counter: Arc<Counter>,
    wake: Arc<Wake>,
}

fn start(cols: u16, rows: u16) -> Started {
    let (output, output_rx) = unbounded_channel();
    let counter = Arc::new(Counter::default());

    let wake = Arc::new(Wake {
        observer: Arc::clone(&counter) as Arc<dyn TerminalObserver>,
        pending: AtomicBool::new(false),
    });

    let input = Arc::new(Mutex::new(Vec::new()));

    let pty = ScriptedPty {
        output: output_rx,
        buffered: Vec::new(),
        input: Arc::clone(&input),
    };

    let view = View::start(pty, cols, rows, Arc::clone(&wake)).expect("the engine starts");

    Started {
        view,
        output,
        input,
        counter,
        wake,
    }
}

/// Frames until one satisfies `done`, since the engine parses on its own
/// task.
fn frame_until(view: &mut View, done: impl Fn(&TerminalFrame) -> bool) -> TerminalFrame {
    let deadline = Instant::now() + Duration::from_secs(5);

    loop {
        let frame = view.frame();

        if done(&frame) {
            return frame;
        }

        assert!(Instant::now() < deadline, "the engine never showed it");

        sleep(Duration::from_millis(10));
    }
}

fn row_text(frame: &TerminalFrame, row: u16) -> String {
    let Some(line) = frame.lines.iter().find(|line| line.row == row) else {
        return String::new();
    };

    let mut text = String::new();
    let mut col = 0;

    for run in &line.runs {
        text.extend(repeat_n(' ', usize::from(run.col - col)));
        text.push_str(&run.text);

        col = run.col + run.cells;
    }

    text
}

#[test]
fn a_frame_has_resolved_colors_and_leaves_blank_cells_out() {
    let Started {
        mut view, output, ..
    } = start(20, 4);

    output
        .send(b"\x1b[31mred\x1b[0m   \x1b[7minv\x1b[0m".to_vec())
        .unwrap();

    let frame = frame_until(&mut view, |frame| row_text(frame, 0).contains("inv"));
    let runs = &frame.lines.iter().find(|line| line.row == 0).unwrap().runs;

    let red = runs.iter().find(|run| run.text == "red").unwrap();
    let inverse = runs.iter().find(|run| run.text == "inv").unwrap();

    assert_eq!((red.col, red.cells, red.bg), (0, 3, None));
    assert_ne!(red.fg, frame.foreground);

    // Inverse draws the default background as text over the default
    // foreground.
    assert_eq!(inverse.col, 6);
    assert_eq!(inverse.fg, frame.background);
    assert_eq!(inverse.bg, Some(frame.foreground));

    // The three spaces between them draw nothing.
    assert_eq!(runs.len(), 2);
}

#[test]
fn a_double_width_character_gets_a_run_of_its_own() {
    let Started {
        mut view, output, ..
    } = start(20, 4);

    output.send("a中b".as_bytes().to_vec()).unwrap();

    let frame = frame_until(&mut view, |frame| row_text(frame, 0).contains('b'));

    let runs: Vec<&TerminalRun> = frame
        .lines
        .iter()
        .find(|line| line.row == 0)
        .unwrap()
        .runs
        .iter()
        .collect();

    let cells: Vec<(u16, u16, &str)> = runs
        .iter()
        .map(|run| (run.col, run.cells, run.text.as_str()))
        .collect();

    assert_eq!(cells, [(0, 1, "a"), (1, 2, "中"), (3, 1, "b")]);
}

#[test]
fn later_frames_carry_only_rows_that_changed() {
    let Started {
        mut view, output, ..
    } = start(20, 4);

    let first = view.frame();

    assert!(first.full);
    assert_eq!(first.lines.len(), 4);

    output.send(b"one\r\ntwo".to_vec()).unwrap();

    let written = frame_until(&mut view, |frame| row_text(frame, 1) == "two");

    assert!(!written.full);

    let idle = view.frame();

    assert!(!idle.full);
    assert!(idle.lines.is_empty());

    output.send(b"\r\nthree".to_vec()).unwrap();

    let next = frame_until(&mut view, |frame| row_text(frame, 2) == "three");

    assert!(!next.full);
    assert!(next.lines.iter().all(|line| line.row != 0));
}

#[test]
fn output_reports_a_change_once_until_a_frame_is_read() {
    let Started {
        mut view,
        output,
        counter,
        wake,
        ..
    } = start(20, 4);

    output.send(b"a".to_vec()).unwrap();

    frame_until(&mut view, |frame| row_text(frame, 0) == "a");

    // Reading a frame re-arms the report, as the handle does.
    let before = counter.0.load(Ordering::Relaxed);

    wake.pending.store(false, Ordering::Release);
    view.frame();

    for _ in 0..20 {
        output.send(b"b".to_vec()).unwrap();
    }

    sleep(Duration::from_millis(200));

    assert_eq!(counter.0.load(Ordering::Relaxed) - before, 1);
}

/// Input until it holds `expected`, since the view writes on its own task.
fn input_until(input: &Mutex<Vec<u8>>, expected: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(5);

    while input.lock().as_slice() != expected {
        assert!(
            Instant::now() < deadline,
            "sent {:?}, expected {expected:?}",
            input.lock()
        );

        sleep(Duration::from_millis(10));
    }
}

fn chord(key: &str, control: bool, alt: bool) -> TerminalKeyInput {
    TerminalKeyInput {
        key: key.to_owned(),
        text: Some(key.to_owned()),
        shift: false,
        control,
        alt,
        command: false,
    }
}

#[test]
fn a_control_chord_sends_its_control_byte_even_with_its_character() {
    let Started {
        mut view, input, ..
    } = start(20, 4);

    assert_eq!(view.send_key(&chord("c", true, false)), KeyResult::Sent);

    input_until(&input, b"\x03");
}

#[test]
fn an_option_chord_sends_escape_and_its_character() {
    let Started {
        mut view, input, ..
    } = start(20, 4);

    assert_eq!(view.send_key(&chord("b", false, true)), KeyResult::Sent);

    input_until(&input, b"\x1bb");
}

#[test]
fn scrolling_through_history_redraws_every_row_and_reports_it() {
    let Started {
        mut view,
        output,
        counter,
        wake,
        ..
    } = start(20, 4);

    let lines: String = (1..=20).map(|n| format!("{n}\r\n")).collect();

    output.send(lines.into_bytes()).unwrap();

    frame_until(&mut view, |frame| row_text(frame, 2) == "20");

    let before = counter.0.load(Ordering::Relaxed);

    wake.pending.store(false, Ordering::Release);

    assert!(
        view.session
            .apply_scroll(SurfaceCell { col: 0, row: 0 }, 3, ModifiersState::empty())
    );

    let scrolled = frame_until(&mut view, |frame| frame.full);

    assert_eq!(row_text(&scrolled, 0), "15");
    assert!(counter.0.load(Ordering::Relaxed) > before);
}
