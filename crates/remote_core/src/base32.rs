//! Crockford base32: no `I L O U`, so codes read aloud or typed from a
//! screen survive the usual confusions.

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Encode `bytes` most significant bit first. Callers pass whole multiples
/// of 5 bytes, so no padding bits are emitted.
pub(crate) fn encode(bytes: &[u8]) -> String {
    debug_assert_eq!(bytes.len() % 5, 0);

    let mut out = String::with_capacity(bytes.len() * 8 / 5);

    for chunk in bytes.chunks(5) {
        let bits = chunk.iter().fold(0u64, |acc, &b| acc << 8 | u64::from(b));

        for shift in (0..8).rev() {
            out.push(ALPHABET[(bits >> (shift * 5)) as usize & 31] as char);
        }
    }

    out
}

/// The canonical symbol for a typed character. Case is ignored, and `I`/`L`
/// read as `1` and `O` as `0`, which is what a user copying a code means.
pub(crate) fn canonical(c: char) -> Option<char> {
    let c = match c.to_ascii_uppercase() {
        'I' | 'L' => '1',
        'O' => '0',
        c => c,
    };

    // `as u8` truncates, so a non-ASCII character could alias a symbol.
    (c.is_ascii() && ALPHABET.contains(&(c as u8))).then_some(c)
}
