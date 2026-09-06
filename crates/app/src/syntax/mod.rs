use std::ffi::{CStr, c_void};
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt as _;
use std::path::PathBuf;
use std::{mem, slice, str};

use anyhow::{Context as _, Result, anyhow, bail};
use gpui::SharedString;
use gpui_component::highlighter::{LanguageConfig, LanguageRegistry};
use tree_sitter::Parser;
use tree_sitter_language::LanguageFn;
#[cfg(windows)]
use windows_sys::Win32::Foundation::HMODULE;
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::utils::get_exe_dir;

/// The bundle is loaded rather than linked so the parser tables can be dropped
/// from a build that does not want them, which is why the loader is spelled
/// out per platform instead of taken from a crate.
#[cfg(windows)]
type ModuleHandle = HMODULE;
#[cfg(unix)]
type ModuleHandle = *mut c_void;

#[cfg(windows)]
const BUNDLE_FILE: &str = "tree_sitter.dll";
#[cfg(target_os = "macos")]
const BUNDLE_FILE: &str = "libtree_sitter.dylib";
#[cfg(all(unix, not(target_os = "macos")))]
const BUNDLE_FILE: &str = "libtree_sitter.so";

const ABI_VERSION: u32 = 1;
const MAX_LANGUAGES: u32 = 128;

type LanguageBuilder = unsafe extern "C" fn() -> *const ();
type AbiVersionFn = unsafe extern "system" fn() -> u32;
type LanguageCountFn = unsafe extern "system" fn() -> u32;
type LanguageAtFn = unsafe extern "system" fn(u32, *mut RawLanguageDescriptor) -> u32;
type LoadedFn = unsafe extern "system" fn() -> isize;

#[derive(Clone, Copy)]
#[repr(C)]
struct RawSlice {
    data: *const u8,
    len: usize,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct RawLanguageDescriptor {
    name: RawSlice,
    aliases: RawSlice,
    injection_languages: RawSlice,
    language: Option<LanguageBuilder>,
    highlights: RawSlice,
    injections: RawSlice,
    locals: RawSlice,
}

pub(crate) fn register_languages() -> Result<usize> {
    let (module, path) = load_library()?;

    // LoadLibrary keeps the module mapped until FreeLibrary is called. No
    // matching call is made because every registered Language retains pointers
    // into the parser tables for the remainder of the process.
    let abi_version: AbiVersionFn = unsafe {
        mem::transmute::<LoadedFn, AbiVersionFn>(
            symbol(module, c"nmt_tree_sitter_abi_version")
                .with_context(|| format!("{BUNDLE_FILE} has no ABI version export"))?,
        )
    };
    let language_count: LanguageCountFn = unsafe {
        mem::transmute::<LoadedFn, LanguageCountFn>(
            symbol(module, c"nmt_tree_sitter_language_count")
                .with_context(|| format!("{BUNDLE_FILE} has no language count export"))?,
        )
    };
    let language_at: LanguageAtFn = unsafe {
        mem::transmute::<LoadedFn, LanguageAtFn>(
            symbol(module, c"nmt_tree_sitter_language")
                .with_context(|| format!("{BUNDLE_FILE} has no language export"))?,
        )
    };

    let actual_abi = unsafe { abi_version() };
    if actual_abi != ABI_VERSION {
        bail!(
            "{} uses ABI version {actual_abi}, expected {ABI_VERSION}",
            path.display()
        );
    }

    let count = unsafe { language_count() };
    if count > MAX_LANGUAGES {
        bail!(
            "{} reports an invalid language count of {count}",
            path.display()
        );
    }

    let mut languages = Vec::with_capacity(count as usize);
    for index in 0..count {
        let mut raw = mem::MaybeUninit::<RawLanguageDescriptor>::uninit();
        if unsafe { language_at(index, raw.as_mut_ptr()) } == 0 {
            bail!("{} rejected language index {index}", path.display());
        }
        // A successful call initializes every field in the descriptor.
        let raw = unsafe { raw.assume_init() };
        languages.push(config(raw).with_context(|| {
            format!(
                "{} returned an invalid language at index {index}",
                path.display()
            )
        })?);
    }

    let registry = LanguageRegistry::singleton();
    let mut registered = 0;
    for (aliases, config) in languages {
        registry.register(&config.name, &config);
        registered += 1;
        for alias in aliases {
            registry.register(&alias, &config);
        }
    }

    Ok(registered)
}

#[cfg(windows)]
fn load_library() -> Result<(ModuleHandle, PathBuf)> {
    let path = get_exe_dir().join(BUNDLE_FILE);
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: the name is NUL-terminated and outlives the call.
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        return Err(anyhow!(
            "cannot load {BUNDLE_FILE} beside the executable: {}",
            io::Error::last_os_error()
        ));
    }

    Ok((module, path))
}

#[cfg(unix)]
fn load_library() -> Result<(ModuleHandle, PathBuf)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let path = get_exe_dir().join(BUNDLE_FILE);
    let name = CString::new(path.as_os_str().as_bytes()).context("bundle path holds a NUL")?;
    // `RTLD_LOCAL` keeps the parser symbols out of the global namespace, where
    // they would otherwise be candidates for every later lookup in the process.
    // SAFETY: the name is NUL-terminated and outlives the call.
    let module = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    if module.is_null() {
        // `dlopen` reports through `dlerror`, not `errno`.
        // SAFETY: the pointer is owned by the loader and read before any other
        // call that could replace it.
        let reason = unsafe { libc::dlerror() };
        let reason = if reason.is_null() {
            String::from("unknown error")
        } else {
            unsafe { CStr::from_ptr(reason) }
                .to_string_lossy()
                .into_owned()
        };

        return Err(anyhow!(
            "cannot load {BUNDLE_FILE} beside the executable: {reason}"
        ));
    }

    Ok((module, path))
}

/// The address of `name` in an already-loaded bundle.
///
/// The module is deliberately never unloaded: every registered language keeps
/// pointers into the parser tables for the remainder of the process.
#[cfg(windows)]
fn symbol(module: ModuleHandle, name: &CStr) -> Option<LoadedFn> {
    // SAFETY: `module` came from `LoadLibraryW` above and `name` is
    // NUL-terminated.
    unsafe { GetProcAddress(module, name.as_ptr().cast()) }
}

#[cfg(unix)]
fn symbol(module: ModuleHandle, name: &CStr) -> Option<LoadedFn> {
    // SAFETY: `module` came from `dlopen` above and `name` is NUL-terminated.
    let address = unsafe { libc::dlsym(module, name.as_ptr()) };

    (!address.is_null()).then(|| {
        // SAFETY: a non-null `dlsym` result is a code address; the caller
        // transmutes it to the signature the bundle's ABI version pins.
        unsafe { mem::transmute::<*mut c_void, LoadedFn>(address) }
    })
}

fn config(raw: RawLanguageDescriptor) -> Result<(Vec<SharedString>, LanguageConfig)> {
    let name = text(raw.name)?;
    if name.is_empty() {
        bail!("empty language name");
    }

    let builder = raw.language.context("null language builder")?;
    let language = tree_sitter::Language::new(unsafe { LanguageFn::from_raw(builder) });
    Parser::new()
        .set_language(&language)
        .with_context(|| format!("unsupported Tree-sitter ABI for {name}"))?;

    let aliases = list(raw.aliases)?;
    let injection_languages = list(raw.injection_languages)?;
    let config = LanguageConfig::new(
        name,
        language,
        injection_languages,
        text(raw.highlights)?,
        text(raw.injections)?,
        text(raw.locals)?,
    );

    Ok((aliases, config))
}

fn list(raw: RawSlice) -> Result<Vec<SharedString>> {
    Ok(text(raw)?
        .split('\0')
        .filter(|value| !value.is_empty())
        .map(SharedString::from)
        .collect())
}

fn text(raw: RawSlice) -> Result<&'static str> {
    if raw.len == 0 {
        return Ok("");
    }
    if raw.data.is_null() {
        bail!("null string pointer with non-zero length");
    }

    // The checked DLL version defines every slice as immutable static storage,
    // and the module remains loaded for as long as any returned string can live.
    let bytes = unsafe { slice::from_raw_parts(raw.data, raw.len) };
    str::from_utf8(bytes).context("language data is not UTF-8")
}

const _: () = {
    assert!(mem::size_of::<*const c_void>() == mem::size_of::<LanguageBuilder>());
};
