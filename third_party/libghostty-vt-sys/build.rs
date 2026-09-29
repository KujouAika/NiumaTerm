use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pinned ghostty commit. Update this to pull a newer version.
const GHOSTTY_REPO: &str = "https://github.com/ghostty-org/ghostty.git";
const GHOSTTY_COMMIT: &str = "b0c421fcd2e290629d4285c181b52fe2f2095f06";

/// Identifier for the locally-applied ghostty source patches. Bump this whenever
/// [`patch_ghostty_source`] changes so a cached clone is re-fetched and re-patched.
/// Folded into the fetch stamp alongside `GHOSTTY_COMMIT`.
const GHOSTTY_PATCH_VERSION: &str = "reflow-trim-blank-v9-grow-cursor-y-v3-kitty-screen-pos-v3-blockset-v5-alt-primary-v1-block-vt-v1-baseb0c421f";
const PREBUILT_ENV: &str = "NMT_USE_PREBUILT_LIBGHOSTTY";

#[derive(Clone, Copy)]
enum LinkMode {
    Dynamic,
    Static,
}

impl LinkMode {
    fn current() -> Self {
        if cfg!(feature = "link-dynamic") {
            Self::Dynamic
        } else {
            Self::Static
        }
    }

    fn artifact_kind(self) -> &'static str {
        match self {
            Self::Dynamic => "shared library",
            Self::Static => "static library",
        }
    }

    fn matches_library(self, target: &str, file_name: &str) -> bool {
        match self {
            Self::Dynamic => {
                if target.contains("darwin") {
                    file_name.starts_with("libghostty-vt") && file_name.ends_with(".dylib")
                } else if target.contains("windows") {
                    file_name == "ghostty-vt.lib"
                        || file_name == "ghostty-vt.dll"
                        || file_name == "libghostty-vt.dll.lib"
                        || file_name == "libghostty-vt.dll.a"
                } else {
                    file_name == "libghostty-vt.so" || file_name.starts_with("libghostty-vt.so.")
                }
            }
            Self::Static => {
                if target.contains("windows") {
                    file_name == "ghostty-vt-static.lib"
                } else {
                    file_name == "libghostty-vt.a"
                }
            }
        }
    }

    #[cfg(feature = "pkg-config")]
    fn pkg_config_name(self) -> &'static str {
        match self {
            Self::Dynamic => "libghostty-vt",
            Self::Static => "libghostty-vt-static",
        }
    }
}

fn main() {
    // docs.rs has no Zig toolchain. The checked-in bindings in src/bindings.rs
    // are enough for generating documentation, so skip the entire native
    // build when running under docs.rs.
    if env::var("DOCS_RS").is_ok() {
        return;
    }

    let link_mode = LinkMode::current();

    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SYS_OPTIMIZE");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_INSTALL_DIR");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_STATIC_DEPS_DIR");
    println!("cargo:rerun-if-env-changed=GHOSTTY_SOURCE_DIR");
    println!("cargo:rerun-if-env-changed=GHOSTTY_ZIG_SYSTEM_DIR");
    println!("cargo:rerun-if-env-changed={PREBUILT_ENV}");
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=HOST");
    println!("cargo:rerun-if-env-changed=OPT_LEVEL");
    println!("cargo:rerun-if-changed=build.rs");

    if let Ok(dir) = env::var("LIBGHOSTTY_VT_INSTALL_DIR") {
        assert!(
            !dir.is_empty(),
            "LIBGHOSTTY_VT_INSTALL_DIR must not be empty when set"
        );

        link_install_prefix_impl(link_mode, PathBuf::from(dir), &[]);

        return;
    }

    // An explicit source override should stay authoritative even when the
    // pkg-config feature is enabled, so local Ghostty checkouts remain easy to
    // test against.
    if env::var_os("GHOSTTY_SOURCE_DIR").is_some() {
        build_vendored(link_mode);
        return;
    }

    if env_flag_enabled(PREBUILT_ENV) {
        link_prebuilt(link_mode);
        return;
    }

    // When the pkg-config feature is enabled, prefer an installed library over
    // fetching Ghostty. libghostty is pre-1.0, so this crate intentionally does
    // not promise compatibility with every installed C API revision.
    #[cfg(feature = "pkg-config")]
    if try_pkg_config(link_mode) {
        return;
    }

    build_vendored(link_mode);
}

fn env_flag_enabled(name: &str) -> bool {
    env::var(name).is_ok_and(|value| {
        !matches!(
            value.as_str(),
            "" | "0" | "false" | "False" | "FALSE" | "no" | "No" | "NO" | "off" | "Off" | "OFF"
        )
    })
}

/// Build libghostty-vt from source via zig. The zig build itself generates
/// shared and static artifacts plus pkg-config files in `share/pkgconfig/`.
fn build_vendored(link_mode: LinkMode) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set"));
    let target = env::var("TARGET").expect("TARGET must be set");
    let host = env::var("HOST").expect("HOST must be set");

    // Locate ghostty source: env override > fetch into OUT_DIR.
    let ghostty_dir = match env::var("GHOSTTY_SOURCE_DIR") {
        Ok(dir) => {
            let p = PathBuf::from(dir);
            assert!(
                p.join("build.zig").exists(),
                "GHOSTTY_SOURCE_DIR does not contain build.zig: {}",
                p.display()
            );
            p
        }
        Err(_) => fetch_ghostty(&out_dir),
    };

    // Build libghostty-vt via zig.
    let install_prefix = out_dir.join("ghostty-install");
    let zig_cache_dir = out_dir.join("zig-cache");
    let zig_global_cache_dir = out_dir.join("zig-global-cache");

    let optimize = zig_optimize_mode();

    let mut build = Command::new("zig");

    // Zig's std.http proxy support mangles CONNECT-style HTTPS proxying (the
    // dep CDN answers 400 through a local proxy while direct fetches succeed),
    // so package fetching must bypass any ambient proxy configuration.
    build
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .arg("build")
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg("-Demit-xcframework=false")
        .arg("-Dapp-runtime=none")
        .arg("--prefix")
        .arg(&install_prefix)
        .arg("--cache-dir")
        .arg(&zig_cache_dir)
        .current_dir(&ghostty_dir);

    // Package managers can provide Ghostty's Zig package cache ahead of time
    // and ask Zig to resolve packages from that immutable store path instead
    // of fetching during this Cargo build script.
    if let Ok(dir) = env::var("GHOSTTY_ZIG_SYSTEM_DIR") {
        assert!(
            !dir.is_empty(),
            "GHOSTTY_ZIG_SYSTEM_DIR must not be empty when set"
        );

        let zig_system_dir = PathBuf::from(dir);

        assert!(
            zig_system_dir.exists(),
            "GHOSTTY_ZIG_SYSTEM_DIR does not exist: {}",
            zig_system_dir.display()
        );

        build
            .arg("--system")
            .arg(&zig_system_dir)
            .arg("--global-cache-dir")
            .arg(&zig_global_cache_dir);
    }

    configure_zig_target(&mut build, &target, &host);

    run(build, "zig build");

    let lib_dir = install_prefix.join("lib");
    let include_dir = install_prefix.join("include");
    let search_dirs = library_search_dirs(&target, &install_prefix);

    warn_unused_xcframework(&lib_dir);

    let has_requested_library = search_dirs.iter().any(|dir| {
        std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", dir.display()))
            .any(|entry| {
                let entry = entry.unwrap_or_else(|error| {
                    panic!("failed to read entry from {}: {error}", dir.display())
                });

                let file_name = entry.file_name();

                let Some(file_name) = file_name.to_str() else {
                    return false;
                };

                link_mode.matches_library(&target, file_name)
            })
    });

    assert!(
        has_requested_library,
        "expected libghostty-vt {} in one of {:?}",
        link_mode.artifact_kind(),
        search_dirs
    );

    assert!(
        include_dir.join("ghostty").join("vt.h").exists(),
        "expected header at {}",
        include_dir.join("ghostty").join("vt.h").display()
    );

    emit_link_metadata(link_mode, &target, &search_dirs);
    emit_windows_static_dependency_links(link_mode, &target, &[zig_cache_dir.join("o")]);
    emit_include_metadata(&[include_dir]);
}

fn link_prebuilt(link_mode: LinkMode) {
    assert!(
        matches!(link_mode, LinkMode::Static),
        "prebuilt libghostty-vt only supports the default static link mode; unset {PREBUILT_ENV} to build from source"
    );

    let target_path = env::var("TARGET").expect("TARGET must be set");
    let target = target_path.as_str();
    let install_prefix = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("prebuilt")
        .join(target);

    println!("cargo:rerun-if-changed={}", install_prefix.display());

    assert!(
        install_prefix.exists(),
        "prebuilt libghostty-vt for {target} not found at {}; unset {PREBUILT_ENV} to build from source or generate the prebuilt package",
        install_prefix.display()
    );

    let static_deps_dir = install_prefix.join("lib");

    link_install_prefix_impl(link_mode, install_prefix, &[static_deps_dir]);
}

fn link_install_prefix_impl(
    link_mode: LinkMode,
    install_prefix: PathBuf,
    extra_dependency_roots: &[PathBuf],
) {
    let target = env::var("TARGET").expect("TARGET must be set");
    let include_dir = install_prefix.join("include");
    let search_dirs = library_search_dirs(&target, &install_prefix);

    assert!(
        include_dir.join("ghostty").join("vt.h").exists(),
        "expected header at {}",
        include_dir.join("ghostty").join("vt.h").display()
    );

    assert!(
        search_dirs
            .iter()
            .any(|dir| has_matching_library(link_mode, &target, dir)),
        "expected libghostty-vt {} in one of {:?}",
        link_mode.artifact_kind(),
        search_dirs
    );

    let mut dependency_roots = Vec::new();

    if let Some(parent) = install_prefix.parent() {
        dependency_roots.push(parent.join(".zig-cache").join("o"));
    }

    dependency_roots.extend(extra_dependency_roots.iter().cloned());

    if let Ok(dir) = env::var("LIBGHOSTTY_VT_STATIC_DEPS_DIR") {
        dependency_roots.push(PathBuf::from(dir));
    }

    emit_link_metadata(link_mode, &target, &search_dirs);
    emit_windows_static_dependency_links(link_mode, &target, &dependency_roots);
    emit_include_metadata(&[include_dir]);
}

fn emit_link_metadata(link_mode: LinkMode, target: &str, search_dirs: &[PathBuf]) {
    for dir in search_dirs {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }

    match link_mode {
        LinkMode::Dynamic => println!("cargo:rustc-link-lib=dylib=ghostty-vt"),
        LinkMode::Static if target.contains("windows") => {
            println!("cargo:rustc-link-lib=static=ghostty-vt-static")
        }
        LinkMode::Static => println!("cargo:rustc-link-lib=static=ghostty-vt"),
    }
}

fn has_matching_library(link_mode: LinkMode, target: &str, dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", dir.display()))
        .any(|entry| {
            let entry = entry.unwrap_or_else(|error| {
                panic!("failed to read entry from {}: {error}", dir.display())
            });
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                return false;
            };

            link_mode.matches_library(target, file_name)
        })
}

fn emit_windows_static_dependency_links(link_mode: LinkMode, target: &str, roots: &[PathBuf]) {
    if !matches!(link_mode, LinkMode::Static) || !target.contains("windows") {
        return;
    }

    for dependency in ["simdutf", "highway"] {
        let library =
            find_newest_library(roots, &format!("{dependency}.lib")).unwrap_or_else(|| {
                panic!(
                    "expected {dependency}.lib for static Windows linking under one of {roots:?}"
                )
            });

        let library_dir = library
            .parent()
            .unwrap_or_else(|| panic!("{} has no parent directory", library.display()));

        println!("cargo:rustc-link-search=native={}", library_dir.display());
        println!("cargo:rustc-link-lib=static={dependency}");
    }
}

fn find_newest_library(roots: &[PathBuf], file_name: &str) -> Option<PathBuf> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;

    for root in roots.iter().filter(|root| root.exists()) {
        find_library_recursive(root, file_name, &mut newest);
    }

    newest.map(|(_, path)| path)
}

fn find_library_recursive(
    dir: &Path,
    file_name: &str,
    newest: &mut Option<(std::time::SystemTime, PathBuf)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            find_library_recursive(&path, file_name, newest);
            continue;
        }

        if path.file_name().and_then(|name| name.to_str()) != Some(file_name) {
            continue;
        }

        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        if newest
            .as_ref()
            .is_none_or(|(current, _)| modified > *current)
        {
            *newest = Some((modified, path));
        }
    }
}

fn warn_unused_xcframework(lib_dir: &Path) {
    let xcframework = lib_dir.join("ghostty-vt.xcframework");
    if xcframework.exists() {
        println!(
            "cargo:warning=unused libghostty-vt XCFramework emitted at {}; Cargo links the dylib or archive directly",
            xcframework.display()
        );
    }
}

#[cfg(feature = "pkg-config")]
fn try_pkg_config(link_mode: LinkMode) -> bool {
    let mut config = pkg_config::Config::new();

    let lib = match link_mode {
        LinkMode::Dynamic => config.probe(link_mode.pkg_config_name()),
        LinkMode::Static => config
            .statik(true)
            .cargo_metadata(false)
            .probe(link_mode.pkg_config_name()),
    };

    let lib = match lib {
        Ok(lib) => lib,
        Err(_) => return false,
    };

    if let LinkMode::Static = link_mode {
        emit_static_pkg_config_metadata(&lib);
    }

    emit_include_metadata(&lib.include_paths);

    true
}

#[cfg(feature = "pkg-config")]
fn emit_static_pkg_config_metadata(lib: &pkg_config::Library) {
    for path in &lib.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }

    for path in &lib.link_files {
        if let Some(parent) = path.parent() {
            println!("cargo:rustc-link-search=native={}", parent.display());
        }
    }

    for path in &lib.framework_paths {
        println!("cargo:rustc-link-search=framework={}", path.display());
    }

    for framework in &lib.frameworks {
        println!("cargo:rustc-link-lib=framework={framework}");
    }

    println!("cargo:rustc-link-lib=static=ghostty-vt");

    for library in &lib.libs {
        if library != "ghostty-vt" {
            println!("cargo:rustc-link-lib={library}");
        }
    }

    for args in &lib.ld_args {
        if !args.is_empty() {
            println!("cargo:rustc-link-arg=-Wl,{}", args.join(","));
        }
    }
}

fn emit_include_metadata(include_paths: &[PathBuf]) {
    if include_paths.is_empty() {
        return;
    }

    let joined = env::join_paths(include_paths)
        .unwrap_or_else(|error| panic!("failed to join include paths for cargo metadata: {error}"));
    println!("cargo:include={}", joined.to_string_lossy());
}

/// Decide which Zig `OptimizeMode` to pass to `zig build`.
///
/// The `LIBGHOSTTY_VT_SYS_OPTIMIZE` environment variable overrides this unconditionally; accepted
/// values are the four Zig `OptimizeMode` names (`Debug`, `ReleaseSafe`, `ReleaseFast`,
/// `ReleaseSmall`).
///
/// Defaults to `ReleaseFast` for optimized builds. If `OPT_LEVEL` is `0` (the `dev` profile),
/// `Debug` mode is used; `s`/`z` map to `ReleaseSmall`. The decision keys off `OPT_LEVEL`
/// because cargo's `DEBUG` env var reflects debug-*info* — this workspace ships
/// `[profile.release] debug = "full"`, and keying off `DEBUG` compiled the VT engine
/// unoptimized in release builds (a ~2000× slower parse path).
fn zig_optimize_mode() -> &'static str {
    if let Ok(override_mode) = env::var("LIBGHOSTTY_VT_SYS_OPTIMIZE") {
        return match override_mode.as_str() {
            "Debug" => "Debug",
            "ReleaseSafe" => "ReleaseSafe",
            "ReleaseFast" => "ReleaseFast",
            "ReleaseSmall" => "ReleaseSmall",
            other => panic!(
                "LIBGHOSTTY_VT_SYS_OPTIMIZE must be one of Debug, ReleaseSafe, ReleaseFast, ReleaseSmall (got '{other}')"
            ),
        };
    }

    match env::var("OPT_LEVEL").as_deref() {
        // Windows: never Zig Debug. Zig 0.15's self-hosted x86_64 backend
        // (the Debug-mode default) emits a COFF for the grown ghostty zcu
        // object that every MSVC-side reader rejects (llvm-objcopy
        // "SymbolTableIndex out of range"; lib.exe/dumpbin LNK1106 seek past
        // EOF), so the archive cannot be indexed or linked at all.
        // ReleaseSafe keeps assertions while forcing the LLVM backend.
        // Revisit when a Zig release fixes the self-hosted COFF writer.
        Ok("0") if env::var("TARGET").is_ok_and(|t| t.contains("windows")) => "ReleaseFast",
        Ok("0") => "Debug",
        Ok("s") | Ok("z") => "ReleaseSmall",
        _ => "ReleaseFast",
    }
}

/// Clone ghostty at the pinned commit into OUT_DIR/ghostty-src.
/// Reuses an existing clone if the commit matches.
fn fetch_ghostty(out_dir: &Path) -> PathBuf {
    let src_dir = out_dir.join("ghostty-src");
    let stamp = src_dir.join(".ghostty-commit");
    // The stamp couples the upstream commit with the local patch revision so that
    // bumping either re-fetches and re-patches a clean tree.
    let stamp_id = format!("{GHOSTTY_COMMIT}:{GHOSTTY_PATCH_VERSION}");

    // Skip fetch if we already have the right commit + patch revision.
    if stamp.exists()
        && let Ok(existing) = std::fs::read_to_string(&stamp)
        && existing.trim() == stamp_id
    {
        return src_dir;
    }

    // Clean and clone fresh.
    if src_dir.exists() {
        std::fs::remove_dir_all(&src_dir)
            .unwrap_or_else(|e| panic!("failed to remove {}: {e}", src_dir.display()));
    }

    eprintln!("Fetching ghostty {GHOSTTY_COMMIT} ...");

    let mut clone = Command::new("git");
    clone
        .arg("clone")
        .arg("--filter=blob:none")
        .arg("--no-checkout")
        .arg(GHOSTTY_REPO)
        .arg(&src_dir);
    run(clone, "git clone ghostty");

    let mut checkout = Command::new("git");
    checkout
        .arg("checkout")
        .arg(GHOSTTY_COMMIT)
        .current_dir(&src_dir);
    run(checkout, "git checkout ghostty commit");

    patch_ghostty_source(&src_dir);

    std::fs::write(&stamp, &stamp_id).unwrap_or_else(|e| panic!("failed to write stamp: {e}"));

    src_dir
}

/// Apply the sorted local patches to a fresh Ghostty checkout via `git apply`.
/// Platform-specific changes self-gate in Zig so one patch series can build on
/// every supported target.
///
/// A patch that no longer applies (after a `GHOSTTY_COMMIT` bump) fails the build with
/// a clear message; regenerate it and bump `GHOSTTY_PATCH_VERSION`.
fn patch_ghostty_source(src_dir: &Path) {
    let patch_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("patches");
    println!("cargo:rerun-if-changed={}", patch_dir.display());

    let mut patches: Vec<PathBuf> = std::fs::read_dir(&patch_dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", patch_dir.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("patch"))
        .collect();
    patches.sort();

    for patch in &patches {
        println!("cargo:rerun-if-changed={}", patch.display());
        let mut apply = Command::new("git");
        apply
            .arg("apply")
            .arg("--whitespace=nowarn")
            .arg(patch)
            .current_dir(src_dir);
        run(apply, &format!("git apply {}", patch.display()));
    }
}

fn run(mut command: Command, context: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to execute {context}: {error}"));
    assert!(status.success(), "{context} failed with status {status}");
}

/// Returns directories to search for the built library artifact.
/// On Windows, Zig may place the DLL in `bin/` and the import lib in `lib/`,
/// so both are included.
fn library_search_dirs(target: &str, install_prefix: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![install_prefix.join("lib")];
    if target.contains("windows") {
        dirs.push(install_prefix.join("bin"));
    }
    dirs
}

fn zig_target(target: &str) -> String {
    let value = match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "aarch64-apple-darwin" => "aarch64-macos-none",
        "x86_64-apple-darwin" => "x86_64-macos-none",
        "aarch64-apple-ios" => "aarch64-ios",
        "aarch64-apple-ios-sim" => "aarch64-ios-simulator",
        "x86_64-pc-windows-gnu" => "x86_64-windows-gnu",
        "aarch64-pc-windows-gnullvm" => "aarch64-windows-gnu",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        other => panic!("unsupported Rust target for vendored build: {other}"),
    };
    value.to_owned()
}

fn configure_zig_target(build: &mut Command, target: &str, host: &str) {
    let is_windows_target = target.contains("windows");
    let is_apple_target = target.contains("apple");

    // Windows binaries run beyond the build machine. Leaving the Zig target
    // implicit can place AVX-512 in compiler_rt, including the memset used by
    // the static CRT before main. x86_64 Windows intentionally requires AVX2.
    //
    // Apple binaries ship too, as the release app and as the checked-in
    // prebuilt archive. An implicit target is the build machine's own CPU
    // model, so an archive built on a newer Apple chip could use instructions
    // an older supported Mac lacks; naming the target resolves the CPU to the
    // architecture's baseline for the OS (Apple M1 for arm64 macOS).
    if target != host || is_windows_target || is_apple_target {
        let zig_target = zig_target(target);
        build.arg(format!("-Dtarget={zig_target}"));
    }

    if is_windows_target {
        build.arg("-Dcpu=baseline+avx2");
    }
}
