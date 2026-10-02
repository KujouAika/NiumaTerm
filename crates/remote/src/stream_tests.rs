use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use nmt_remote_core::frame::kind;
use nmt_terminal::event::{Checkpoint, CheckpointRequest, Msg, OutputSink};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::sync::watch;

use crate::link::{QueueReceiver, SendQueue};
use crate::sessions::TerminalControl;
use crate::stream::StreamFlow;

fn checkpoint(vt: &[u8]) -> Checkpoint {
    Checkpoint {
        vt: vt.to_vec(),
        cols: 80,
        rows: 24,
    }
}

/// Play the pump: take everything queued, report it sent, and return the
/// kinds and sizes in order.
fn drain(queue: &mut QueueReceiver) -> Vec<(u8, usize)> {
    let mut sent = Vec::new();

    while let Some(outbound) = queue.try_recv() {
        sent.push((outbound.kind, outbound.payload.len()));

        if let Some(hook) = &outbound.on_sent {
            hook.sent(outbound.payload.len());
        }
    }

    sent
}

fn next_checkpoint_request(loop_rx: &mut UnboundedReceiver<Msg>) -> CheckpointRequest {
    match loop_rx.try_recv() {
        Ok(Msg::Checkpoint(request)) => request,
        other => panic!("expected a checkpoint request, got {other:?}"),
    }
}

#[tokio::test]
async fn a_flood_drops_output_then_resyncs_with_a_checkpoint() {
    let (messenger, mut loop_rx) = unbounded_channel();

    let (_size, size_rx) = watch::channel((80, 24));

    let (queue, mut queue_rx) = SendQueue::new();

    let control = TerminalControl {
        messenger,
        claimed_remotely: Arc::new(AtomicBool::new(false)),
    };

    let _flow = StreamFlow::attach(3, &control, size_rx, queue);

    // Play the PTY loop: the subscription's checkpoint comes first.
    let Ok(Msg::Subscribe {
        sink: OutputSink(mut sink),
        checkpoint: CheckpointRequest(first),
    }) = loop_rx.try_recv()
    else {
        panic!("attaching subscribes");
    };

    first(Ok(checkpoint(b"attach")));

    assert_eq!(drain(&mut queue_rx), vec![(kind::CHECKPOINT, 6)]);

    // The link stalls: nothing is sent while output keeps coming.
    let chunk = vec![b'x'; 600 * 1024];

    assert!(sink(chunk.clone().into()));
    assert!(sink(chunk.clone().into()), "past the high-water mark");
    assert!(sink(chunk.clone().into()), "dropped while behind");

    // Only the output that fit was queued; the rest was dropped.
    assert_eq!(drain(&mut queue_rx), vec![(kind::OUTPUT, chunk.len())]);

    // Draining below the low-water mark asks the loop for a checkpoint.
    let CheckpointRequest(resync) = next_checkpoint_request(&mut loop_rx);

    // Output parsed before that request runs is in the checkpoint, so it
    // is still dropped.
    assert!(sink(b"covered by the checkpoint".to_vec().into()));

    resync(Ok(checkpoint(b"resync")));

    assert!(sink(b"live".to_vec().into()));

    assert_eq!(
        drain(&mut queue_rx),
        vec![(kind::CHECKPOINT, 6), (kind::OUTPUT, 4)]
    );
}

#[tokio::test]
async fn a_detached_view_unsubscribes_and_hears_no_exit() {
    let (messenger, mut loop_rx) = unbounded_channel();

    let (_size, size_rx) = watch::channel((80, 24));

    let (queue, mut queue_rx) = SendQueue::new();

    let control = TerminalControl {
        messenger,
        claimed_remotely: Arc::new(AtomicBool::new(false)),
    };

    let flow = StreamFlow::attach(5, &control, size_rx, queue);

    let Ok(Msg::Subscribe {
        sink: OutputSink(mut sink),
        ..
    }) = loop_rx.try_recv()
    else {
        panic!("attaching subscribes");
    };

    flow.close();

    assert!(!sink(b"late".to_vec().into()), "the loop drops the sink");

    drop(sink);

    assert!(drain(&mut queue_rx).is_empty());
}
