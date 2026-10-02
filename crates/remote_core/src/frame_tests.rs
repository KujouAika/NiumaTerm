use crate::channel::MAX_PLAINTEXT;
use crate::frame::{
    FLAG_MORE, Frame, HEADER_LEN, MAX_MESSAGE, MAX_PAYLOAD, Message, Reassembler, encode_message,
    kind,
};
use crate::{Error, MAX_NOISE_MESSAGE, TAG_LEN};

fn reassemble(frames: &[Vec<u8>]) -> Vec<Message> {
    let mut reassembler = Reassembler::default();

    frames
        .iter()
        .filter_map(|bytes| reassembler.push(Frame::decode(bytes).unwrap()).unwrap())
        .collect()
}

#[test]
fn a_full_frame_fits_one_noise_message() {
    assert_eq!(HEADER_LEN + MAX_PAYLOAD, MAX_PLAINTEXT);
    assert_eq!(MAX_PLAINTEXT + TAG_LEN, MAX_NOISE_MESSAGE);
}

#[test]
fn messages_split_exactly_at_the_payload_limit() {
    for (len, expected_frames) in [
        (0, 1),
        (MAX_PAYLOAD, 1),
        (MAX_PAYLOAD + 1, 2),
        (3 * MAX_PAYLOAD, 3),
    ] {
        let message: Vec<u8> = (0..len).map(|i| i as u8).collect();
        let frames = encode_message(7, kind::CHECKPOINT, &message);

        assert_eq!(frames.len(), expected_frames, "len {len}");
        assert!(frames.iter().all(|f| f.len() <= MAX_PLAINTEXT));

        for (index, frame) in frames.iter().enumerate() {
            let more = Frame::decode(frame).unwrap().flags & FLAG_MORE != 0;

            assert_eq!(more, index + 1 < frames.len());
        }

        assert_eq!(
            reassemble(&frames),
            vec![Message {
                stream: 7,
                kind: kind::CHECKPOINT,
                payload: message,
            }]
        );
    }
}

#[test]
fn interleaved_streams_reassemble_independently() {
    let big = vec![0xAB; 2 * MAX_PAYLOAD + 5];
    let checkpoint = encode_message(1, kind::CHECKPOINT, &big);
    let input = encode_message(2, kind::INPUT, b"ls\r");

    let frames = vec![
        checkpoint[0].clone(),
        input[0].clone(),
        checkpoint[1].clone(),
        checkpoint[2].clone(),
    ];

    let messages = reassemble(&frames);

    assert_eq!(messages.len(), 2);
    assert_eq!(
        (messages[0].stream, &messages[0].payload[..]),
        (2, &b"ls\r"[..])
    );
    assert_eq!(
        (messages[1].stream, messages[1].payload.len()),
        (1, big.len())
    );
}

#[test]
fn oversized_or_inconsistent_fragments_are_rejected() {
    let mut reassembler = Reassembler::default();

    let chunk = vec![0; MAX_PAYLOAD];

    let mut header = 3u32.to_le_bytes().to_vec();

    header.extend([kind::OUTPUT, FLAG_MORE]);

    let frame = [header, chunk].concat();

    let mut result = Ok(None);

    for _ in 0..=MAX_MESSAGE / MAX_PAYLOAD {
        result = reassembler.push(Frame::decode(&frame).unwrap());

        if result.is_err() {
            break;
        }
    }

    assert!(matches!(
        result,
        Err(Error::TooLarge { limit: MAX_MESSAGE })
    ));

    let mut reassembler = Reassembler::default();

    let first = encode_message(4, kind::OUTPUT, &vec![0; MAX_PAYLOAD + 1]);
    let other_kind = encode_message(4, kind::INPUT, b"[]");

    assert!(
        reassembler
            .push(Frame::decode(&first[0]).unwrap())
            .unwrap()
            .is_none()
    );
    assert!(
        reassembler
            .push(Frame::decode(&other_kind[0]).unwrap())
            .is_err()
    );
    assert!(Frame::decode(&[0; HEADER_LEN - 1]).is_err());
}
