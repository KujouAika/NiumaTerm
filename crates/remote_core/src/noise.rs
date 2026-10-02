use snow::HandshakeState;

use crate::identity::key_array;
use crate::{Error, MAX_NOISE_MESSAGE};

pub(crate) fn write(noise: &mut HandshakeState, payload: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = vec![0; MAX_NOISE_MESSAGE];

    let len = noise.write_message(payload, &mut out)?;

    out.truncate(len);

    Ok(out)
}

pub(crate) fn read(noise: &mut HandshakeState, message: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = vec![0; message.len()];

    let len = noise.read_message(message, &mut out)?;

    out.truncate(len);

    Ok(out)
}

pub(crate) fn remote_static(noise: &HandshakeState) -> Result<[u8; 32], Error> {
    key_array(
        noise
            .get_remote_static()
            .ok_or(Error::Malformed("peer key"))?,
    )
}
