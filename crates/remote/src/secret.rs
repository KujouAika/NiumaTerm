//! Protects the device private key at rest. Whoever reads that key can run
//! commands on every host that trusts the device, so it is sealed to the
//! current OS user rather than with a key compiled into the binary: DPAPI on
//! Windows, a login Keychain item on macOS.

use std::io;
#[cfg(windows)]
use std::{ptr, slice};

#[cfg(target_os = "macos")]
use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
#[cfg(target_os = "macos")]
use aes_gcm::{Aes256Gcm, Key, Nonce};
#[cfg(target_os = "macos")]
use core_foundation::data::CFData;
#[cfg(target_os = "macos")]
use parking_lot::Mutex;
#[cfg(target_os = "macos")]
use security_framework::base::Error as KeychainError;
#[cfg(target_os = "macos")]
use security_framework::item::{ItemAddOptions, ItemAddValue, ItemClass};
#[cfg(target_os = "macos")]
use security_framework::passwords::{PasswordOptions, generic_password};
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

// macOS has no DPAPI counterpart that seals arbitrary bytes, so one random
// AES-256-GCM key lives in the user's login Keychain and seals every secret
// file. Keeping the secrets themselves in files leaves the on-disk layout and
// the per-instance directories of `--testing` launches unchanged, and a single
// Keychain item means at most one access prompt when the code signature of
// the reading binary changes.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "NiumaTerm";

#[cfg(all(target_os = "macos", not(test)))]
const KEYCHAIN_ACCOUNT: &str = "remote-sessions-sealing-key";

// Test binaries get their own item: they are rebuilt with a new code
// signature every time, and reading the application's item would ask the
// user to approve each build.
#[cfg(all(target_os = "macos", test))]
const KEYCHAIN_ACCOUNT: &str = "remote-sessions-sealing-key-tests";

#[cfg(target_os = "macos")]
const NONCE_LEN: usize = 12;

// Security framework status codes, from `SecBase.h`.
#[cfg(target_os = "macos")]
const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;

#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// The sealing key once read, so each secret does not cost a Keychain
/// round trip, and so two threads sealing their first secret at once cannot
/// both generate a key.
#[cfg(target_os = "macos")]
static SEALING_KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);

/// Seal `data` with the Keychain-held key: a random nonce followed by the
/// AES-GCM ciphertext and tag.
#[cfg(target_os = "macos")]
pub(crate) fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
    let cipher = cipher()?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, data)
        .map_err(|_| io::Error::other("sealing a remote-session secret failed"))?;

    let mut sealed = nonce.to_vec();

    sealed.extend_from_slice(&ciphertext);

    Ok(sealed)
}

#[cfg(target_os = "macos")]
pub(crate) fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    let cipher = cipher()?;

    let Some((nonce, ciphertext)) = data.split_at_checked(NONCE_LEN) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "sealed secret is truncated",
        ));
    };

    // A failed tag check also covers a file sealed under a Keychain key that
    // has since been deleted and replaced.
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "sealed secret does not open"))
}

#[cfg(target_os = "macos")]
fn cipher() -> io::Result<Aes256Gcm> {
    let mut cached = SEALING_KEY.lock();

    let key = match *cached {
        Some(key) => key,
        None => *cached.insert(load_or_create_sealing_key()?),
    };

    Ok(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key)))
}

#[cfg(target_os = "macos")]
fn load_or_create_sealing_key() -> io::Result<[u8; 32]> {
    let stored = match read_sealing_key() {
        Ok(stored) => stored,
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => add_sealing_key()?,
        Err(error) => return Err(keychain_error(error)),
    };

    stored.try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the Keychain sealing key has the wrong length",
        )
    })
}

#[cfg(target_os = "macos")]
fn read_sealing_key() -> Result<Vec<u8>, KeychainError> {
    generic_password(PasswordOptions::new_generic_password(
        KEYCHAIN_SERVICE,
        KEYCHAIN_ACCOUNT,
    ))
}

/// Store a fresh random key and return the key the Keychain now holds.
#[cfg(target_os = "macos")]
fn add_sealing_key() -> io::Result<Vec<u8>> {
    let key = Aes256Gcm::generate_key(&mut OsRng);

    let mut options = ItemAddOptions::new(ItemAddValue::Data {
        class: ItemClass::generic_password(),
        data: CFData::from_buffer(&key),
    });

    options
        .set_service(KEYCHAIN_SERVICE)
        .set_account_name(KEYCHAIN_ACCOUNT)
        .set_label("NiumaTerm remote sessions");

    // Add-only rather than add-or-update: another NiumaTerm process that
    // created the item first may already have sealed files with it, and
    // overwriting its key would leave those files unreadable.
    match options.add() {
        Ok(()) => Ok(key.to_vec()),
        Err(error) if error.code() == ERR_SEC_DUPLICATE_ITEM => {
            read_sealing_key().map_err(keychain_error)
        }
        Err(error) => Err(keychain_error(error)),
    }
}

#[cfg(target_os = "macos")]
fn keychain_error(error: KeychainError) -> io::Error {
    io::Error::other(format!("Keychain access failed: {error}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn protect(_data: &[u8]) -> io::Result<Vec<u8>> {
    Err(unsupported())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn unprotect(_data: &[u8]) -> io::Result<Vec<u8>> {
    Err(unsupported())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "remote sessions need secret storage, which this platform does not have yet",
    )
}
