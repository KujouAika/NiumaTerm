//! Protects the device private key at rest. Whoever reads that key can run
//! commands on every host that trusts the device, so it is sealed to the
//! current OS user rather than with a key compiled into the binary.

use std::io;
#[cfg(windows)]
use std::{ptr, slice};

#[cfg(windows)]
use windows_sys::Win32::Foundation::LocalFree;
#[cfg(windows)]
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

/// Seal `data` to the current Windows user with DPAPI.
#[cfg(windows)]
pub(crate) fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = input_blob(data)?;

    let mut output = empty_blob();

    // SAFETY: `input` points at `data` for the duration of the call, and
    // DPAPI allocates `output`, which `take_blob` releases.
    let ok = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };

    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(take_blob(output))
}

#[cfg(windows)]
pub(crate) fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = input_blob(data)?;

    let mut output = empty_blob();

    // SAFETY: as in `protect`.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };

    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(take_blob(output))
}

#[cfg(windows)]
fn input_blob(data: &[u8]) -> io::Result<CRYPT_INTEGER_BLOB> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?,
        pbData: data.as_ptr().cast_mut(),
    })
}

#[cfg(windows)]
fn empty_blob() -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    }
}

/// Copy a DPAPI-allocated blob into Rust memory and free the original.
#[cfg(windows)]
fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if blob.pbData.is_null() {
        return Vec::new();
    }

    // SAFETY: DPAPI returned `cbData` initialized bytes at `pbData`,
    // allocated with LocalAlloc and owned by the caller.
    let bytes = unsafe { slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();

    unsafe { LocalFree(blob.pbData.cast()) };

    bytes
}

// macOS Keychain storage (security-framework generic password) lands with
// the macOS host; until then remote sessions report the platform unsupported.
#[cfg(not(windows))]
pub(crate) fn protect(_data: &[u8]) -> io::Result<Vec<u8>> {
    Err(unsupported())
}

#[cfg(not(windows))]
pub(crate) fn unprotect(_data: &[u8]) -> io::Result<Vec<u8>> {
    Err(unsupported())
}

#[cfg(not(windows))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "remote sessions need secret storage, which this platform does not have yet",
    )
}
