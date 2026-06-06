//! Std-runtime hookup for `no_std` crates promoted to `dylib` via the
//! unstable `cascade-dylib` feature.
//!
//! Background
//! ----------
//!
//! When a consumer runs
//!
//! ```text
//! cargo +nightly build -Z cascade-dylib=cfg-if
//! ```
//!
//! (or has a manifest-declared `[lib].crate-type = ["lib", "dylib"]`
//! workspace lib and runs `-Z cascade-dylib` bare), cargo rewrites
//! `cfg-if`'s `[lib].crate-type` to also emit a `dylib` (see the
//! `Per-package profile override` arm in [`UnitInterner::intern`]).
//! Because a `dylib` is its own final-link unit, it must resolve
//! `#[panic_handler]`, `#[global_allocator]`, and the unwinder's
//! `eh_personality` lang item at link time. A `#![no_std]` crate
//! provides none of these.
//!
//! Solution
//! --------
//!
//! Force-link `std` itself into the qualifying rustc invocation via
//!
//! ```text
//! --extern force:std=<path-to-libstd-*.dylib>
//! --extern force:std=<path-to-libstd-*.rmeta>
//! -Z unstable-options
//! ```
//!
//! With `std` linked, the runtime symbols resolve transitively and the
//! dylib produces successfully. Crucially, the `std` crate is a real
//! toolchain crate available in both `rlib` and `dylib` formats, so
//! rustc's link-format inference resolves it transparently for any
//! downstream consumer regardless of whether the consumer reaches the
//! promoted dylib through a static-rlib or dynamic-dylib edge.
//!
//! An earlier iteration of this machinery synthesized a
//! `granita_std_shim` rlib and force-linked *that* into the dylib. The
//! synthetic shim worked for simple graphs but blew up under
//! `-C prefer-dynamic` once the dep graph reached a promoted dylib via
//! both rlib and dylib edges: rustc rejected the mixed-format paths to
//! the synthetic crate ("cannot satisfy dependencies so
//! `granita_std_shim` only shows up once"). Routing the metadata edge
//! through `std` — a crate that *is* available as both rlib and dylib —
//! sidesteps that link-format-uniformity invariant entirely.
//!
//! Both the `dylib` and `rmeta` paths are required because the shipped
//! standard library uses `-Zembed-metadata=no`: the `.dylib` carries
//! only a metadata stub and rustc demands the `.rmeta` separately.
//!
//! `no_std` detection
//! ------------------
//!
//! A package is treated as `no_std` if any of its lib/bin/example/etc.
//! root source files declares `#![no_std]` at the top of the file
//! (skipping over leading whitespace, line comments, and shebangs). A
//! `#![no_std]` inside a string literal or `/* ... */` block comment
//! would falsely match, but that's pathological — the heuristic
//! mirrors the simple textual scans cargo already uses elsewhere.
//!
//! [`UnitInterner::intern`]: super::unit::UnitInterner::intern

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::compiler::{BuildContext, CompileKind, CrateType, Unit};
use crate::core::manifest::TargetSourcePath;
use crate::core::{Package, Target};
use crate::util::CargoResult;

/// Returns true if this unit's dylib emission needs `std` force-linked
/// in. A unit qualifies when:
///
/// 1. Its profile carries a `crate-type` override (i.e. the
///    `cascade-dylib` machinery fired for this package).
/// 2. The override produces a `dylib` (the rlib emission is left
///    untouched in spirit, though rustc emits both from one
///    invocation; the extra `--extern force:std` is harmless on the
///    rlib because `std` is a real toolchain crate that consumers
///    resolve fluently across formats).
/// 3. The package is `no_std` per [`is_no_std`].
pub fn unit_needs_std_link(
    profile_crate_type: Option<&[CrateType]>,
    target: &Target,
    pkg: &Package,
) -> CargoResult<bool> {
    let Some(crate_types) = profile_crate_type else {
        return Ok(false);
    };
    if !target.is_lib() {
        return Ok(false);
    }
    if !crate_types.contains(&CrateType::Dylib) {
        return Ok(false);
    }
    is_no_std(pkg)
}

/// Returns the platform-appropriate `-C link-arg=...` pair to inject so
/// the resulting dylib's `LC_RPATH` (macOS) / `DT_RUNPATH` (Linux) chain
/// includes `target/<profile>/deps/`. Without this, the dynamic loader
/// searches only `@loader_path` (= `target/<profile>/`) for sibling
/// `@rpath/lib*-<svh>.dylib` references, and the SVH-stamped cascade-
/// promoted dylibs at `target/<profile>/deps/` go missing — manifesting
/// as `dlopen failed` in any consumer that doesn't set
/// `DYLD_FALLBACK_LIBRARY_PATH` in its environment (cargo's `cargo run`
/// codepath does; `cargo install`'d binaries and arbitrary `dlopen` from
/// other tools don't).
///
/// Fires when:
///
/// 1. `-Z cascade-dylib[=...]` is active for this build.
/// 2. The unit being built is a lib target whose effective crate-type
///    contains `dylib` — covers both manifest-declared dylibs (cascade
///    roots) and profile-overridden dylibs (cascade-promoted deps).
///    Both end up with `@rpath/lib*-<svh>.dylib` references that need
///    the same search path.
///
/// Returns an empty Vec on Windows (PE doesn't use rpath; DLLs resolve
/// via the system search path), or when either gate fails. Adding a
/// redundant rpath is harmless on the platforms that do honor it: the
/// dynamic loader just appends another search entry.
pub fn cascade_rpath_args(
    bcx: &BuildContext<'_, '_>,
    unit: &Unit,
) -> CargoResult<Vec<OsString>> {
    if bcx.gctx.cli_unstable().cascade_dylib.is_none() {
        return Ok(Vec::new());
    }
    if !unit.target.is_lib() {
        return Ok(Vec::new());
    }

    let manifest_dylib = unit.target.rustc_crate_types().contains(&CrateType::Dylib);
    let profile_dylib = unit
        .profile
        .crate_type
        .as_deref()
        .map_or(false, |ct| ct.contains(&CrateType::Dylib));
    if !manifest_dylib && !profile_dylib {
        return Ok(Vec::new());
    }

    let triple = bcx.target_data.short_name(&unit.kind);
    let rpath = if triple.contains("-apple-") {
        "@loader_path/deps"
    } else if triple.contains("-linux-")
        || triple.contains("-freebsd")
        || triple.contains("-netbsd")
        || triple.contains("-openbsd")
        || triple.contains("-dragonfly")
    {
        "$ORIGIN/deps"
    } else {
        // Windows (PE) doesn't use rpath; DLLs resolve via the system
        // search path. Cascade-dylib's hot-reload use case hasn't been
        // smoke-tested on Windows; skip rather than emit a flag the
        // MSVC linker would reject.
        return Ok(Vec::new());
    };

    let mut link_arg = OsString::from("link-arg=-Wl,-rpath,");
    link_arg.push(rpath);
    Ok(vec![OsString::from("-C"), link_arg])
}

/// Whether a cascade-promoted dylib should force-load and re-export the C
/// symbols of any native static archive it links. The actual link args are
/// emitted by [`add_native_deps`] at link time, where the build-script
/// output (the `-l static=…` names and their `-L` search dirs) is in hand;
/// this is just the unit-level gate, computed up front for that closure.
///
/// Why it's needed: on macOS, rustc passes `-exported_symbols_list` naming
/// only the crate's own Rust symbols. A `-sys` crate is just `extern`
/// declarations — it never *calls* the C functions — so `ld` pulls zero
/// members from the static archive (static archives load only referenced
/// members), and even force-loaded members would be hidden by that export
/// list. A Rust FFI consumer reached through a *separate* promoted dylib
/// (`zstd-safe`/`zstd`, whose `#[inline]`/generic wrappers monomorphize the
/// `extern` calls into their own codegen unit) then can't resolve them, and
/// the link fails with undefined native symbols — the failure that blocks
/// promoting `-sys` crates to dylibs at all. The fix pairs `-force_load`
/// (pull every archive member in) with `-Wl,-exported_symbol,_*` (macOS
/// `ld` *unions* it with rustc's list, widening exports to cover the C
/// symbols, and keeps them as GC roots). Depth-independent through any
/// wrapper chain, and avoids the link-format diamond that keeping the
/// `links` crate rlib would cause.
///
/// The gate is just "a promoted lib dylib on macOS". The real scoping happens
/// in [`add_native_deps`], which force-loads + re-exports ONLY when the unit's
/// build-script output actually declares a `static=` native lib. So pure-Rust
/// dylibs (the app, iced, …) and `links`-marker crates with no archive (e.g.
/// `rayon-core`) get nothing and keep their `-dead_strip`; only true archive
/// absorbers pay the export glob's no-dead-strip cost. We deliberately do NOT
/// gate on the manifest `links` key: plenty of `-sys` crates emit
/// `rustc-link-lib=static` without declaring `links` (e.g. `openpnp_capture_sys`).
/// ELF is excluded: a shared object exports its globals by default, so a linked
/// archive's symbols cross dylib boundaries unaided. (macOS additionally needs
/// the archive built `-fvisibility=default`, else its symbols are private-extern
/// and unexportable — that's a build-flag concern, not handled here.)
pub fn cascade_needs_native_reexport(bcx: &BuildContext<'_, '_>, unit: &Unit) -> bool {
    if bcx.gctx.cli_unstable().cascade_dylib.is_none() {
        return false;
    }
    if !unit.target.is_lib() {
        return false;
    }
    let manifest_dylib = unit.target.rustc_crate_types().contains(&CrateType::Dylib);
    let profile_dylib = unit
        .profile
        .crate_type
        .as_deref()
        .map_or(false, |ct| ct.contains(&CrateType::Dylib));
    if !manifest_dylib && !profile_dylib {
        return false;
    }
    bcx.target_data.short_name(&unit.kind).contains("-apple-")
}

/// Builds the `--extern force:std=<dylib> --extern force:std=<rmeta>`
/// argument pair for the given `kind`'s sysroot, picking the freshest
/// `libstd-<hash>.{dylib,rmeta}` pair.
///
/// Returns `Ok(None)` if no matching artifacts exist (e.g. a target
/// triple without a shipped libstd, or a host-only sysroot layout that
/// doesn't carry dylibs). Callers fall back to the unmodified
/// invocation in that case; the dylib link will then fail with the
/// usual `panic_handler` diagnostic, which is the right outcome for
/// targets that don't support std.
pub fn std_link_args(bcx: &BuildContext<'_, '_>, kind: CompileKind) -> CargoResult<Vec<OsString>> {
    let info = bcx.target_data.info(kind);
    let libdir = &info.sysroot_target_libdir;
    let Some((dylib, rmeta)) = find_libstd_pair(libdir)? else {
        return Ok(Vec::new());
    };

    let mut dylib_arg = OsString::from("force:std=");
    dylib_arg.push(dylib);
    let mut rmeta_arg = OsString::from("force:std=");
    rmeta_arg.push(rmeta);

    Ok(vec![
        OsString::from("--extern"),
        dylib_arg,
        OsString::from("--extern"),
        rmeta_arg,
    ])
}

/// Scans `libdir` for the matching `libstd-<hash>.dylib` /
/// `libstd-<hash>.so` / `libstd-<hash>.dll` file and the sibling
/// `libstd-<hash>.rmeta`. Returns `None` if either is missing.
fn find_libstd_pair(libdir: &Path) -> CargoResult<Option<(PathBuf, PathBuf)>> {
    let Ok(entries) = fs::read_dir(libdir) else {
        return Ok(None);
    };
    let mut dylib: Option<PathBuf> = None;
    let mut rmeta: Option<PathBuf> = None;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_libstd_artifact(name) {
            continue;
        }
        if name.ends_with(".rmeta") {
            rmeta = Some(path);
        } else if is_dylib_extension(name) {
            dylib = Some(path);
        }
    }
    Ok(match (dylib, rmeta) {
        (Some(d), Some(r)) => Some((d, r)),
        _ => None,
    })
}

fn is_libstd_artifact(name: &str) -> bool {
    // `lib` prefix is platform-specific (Windows `std-*.dll` has no
    // `lib`), but the only platforms cargo supports for dylib output
    // are linux/macos/windows; on Windows the file is `std-<hash>.dll`
    // (no prefix), and on linux/mac it's `libstd-<hash>.{so,dylib}`.
    if let Some(rest) = name.strip_prefix("libstd-") {
        // Reject things like `libstd_detect-*` by checking that the
        // next char is the hash separator (hex digit, not `_`).
        return rest.chars().next().is_some_and(|c| c.is_ascii_hexdigit());
    }
    if let Some(rest) = name.strip_prefix("std-") {
        return rest.chars().next().is_some_and(|c| c.is_ascii_hexdigit());
    }
    false
}

fn is_dylib_extension(name: &str) -> bool {
    name.ends_with(".dylib") || name.ends_with(".so") || name.ends_with(".dll")
}

/// Heuristic `no_std` detection: returns true iff any of `pkg`'s
/// targets has a root source file that declares `#![no_std]` at the
/// top of the file, before any non-trivial code.
///
/// "Top of file" means: after any leading whitespace, line comments,
/// and shebangs, the first attribute-shaped tokens include
/// `#![no_std]`. We don't try to handle block comments or string
/// literals — false positives there are pathological.
fn is_no_std(pkg: &Package) -> CargoResult<bool> {
    for target in pkg.targets() {
        let TargetSourcePath::Path(src) = target.src_path() else {
            continue;
        };
        if file_declares_no_std(src)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn file_declares_no_std(path: &Path) -> CargoResult<bool> {
    // Some test fixtures point at non-existent files; treat that as
    // "not no_std" rather than erroring out.
    let Ok(contents) = fs::read_to_string(path) else {
        return Ok(false);
    };
    Ok(scan_for_no_std(&contents))
}

/// Pure-text scan for a top-of-file `no_std` attribute. Walks the
/// source as a character stream, skipping leading whitespace, line
/// comments (`// ...`), block comments (`/* ... */`, including
/// multi-line and `/*! ... */` outer doc shape), and shebangs
/// (`#!/...`).
///
/// Recognizes both bare `#![no_std]` and `cfg_attr` shapes (e.g.
/// `#![cfg_attr(not(doc), no_std)]` or
/// `#![cfg_attr(all(not(test), not(feature = "std")), no_std)]`)
/// because real-world `no_std` crates frequently use the conditional
/// form. Bracket and parenthesis depth is tracked so multi-line
/// `cfg_attr(...)` invocations are captured fully.
///
/// False positives are bounded: any `#![ ... no_std ... ]` shape in
/// the attribute prelude triggers std injection. Force-linking `std`
/// into a crate that already pulls it in implicitly is harmless.
/// Block-comment nesting (Rust does support `/* /* */ */`) is not
/// recognized — the first `*/` closes the outer comment. This is
/// pathological in real prelude code.
fn scan_for_no_std(src: &str) -> bool {
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => {
                i += 1;
            }
            // Line comment — skip to end of line.
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            // Block comment — skip past the matching `*/`. Includes
            // outer doc-comment shape `/*! ... */`, which pin-project
            // -lite and others use as an alternative to `//!`.
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                if i + 1 < bytes.len() {
                    i += 2;
                } else {
                    // Unterminated block comment — give up. A real
                    // file wouldn't get this far without rustc errors.
                    return false;
                }
            }
            // Inner attribute `#![...]` — collect to matching `]` and
            // probe for `no_std`.
            b'#' if i + 2 < bytes.len() && bytes[i + 1] == b'!' && bytes[i + 2] == b'[' => {
                let start = i;
                i += 3;
                let mut depth = 1_i32;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'[' | b'(' => depth += 1,
                        b']' | b')' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                if contains_no_std_token(&src[start..i]) {
                    return true;
                }
            }
            // Shebang `#!/...` or `#! /...` — skip to end of line.
            b'#' if i + 1 < bytes.len() && bytes[i + 1] == b'!' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            // Anything else is real code (an item, an outer attribute
            // `#[...]`, etc.) — the prelude is over.
            _ => return false,
        }
    }
    false
}

/// True iff `s` contains the identifier `no_std` as a whole token
/// (i.e. surrounded by non-identifier characters on both sides).
fn contains_no_std_token(s: &str) -> bool {
    let needle = "no_std";
    let bytes = s.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut i = 0;
    while i + needle_bytes.len() <= bytes.len() {
        if &bytes[i..i + needle_bytes.len()] == needle_bytes {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after = i + needle_bytes.len();
            let after_ok = after == bytes.len() || !is_ident_byte(bytes[after]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::{is_libstd_artifact, scan_for_no_std};

    #[test]
    fn detects_basic_no_std() {
        assert!(scan_for_no_std("#![no_std]\n"));
    }

    #[test]
    fn detects_after_comments() {
        assert!(scan_for_no_std(
            "// SPDX-License-Identifier: MIT\n\n#![no_std]\n\npub fn x() {}\n"
        ));
    }

    #[test]
    fn detects_after_other_inner_attrs() {
        assert!(scan_for_no_std(
            "#![allow(dead_code)]\n#![no_std]\npub fn x() {}\n"
        ));
    }

    #[test]
    fn rejects_when_absent() {
        assert!(!scan_for_no_std("pub fn x() {}\n"));
    }

    #[test]
    fn rejects_inside_function_body() {
        assert!(!scan_for_no_std("pub fn x() {\n    // #![no_std]\n}\n"));
    }

    #[test]
    fn rejects_in_line_comment() {
        assert!(!scan_for_no_std("// #![no_std]\npub fn x() {}\n"));
    }

    #[test]
    fn skips_shebang() {
        assert!(scan_for_no_std("#!/usr/bin/env rustc\n#![no_std]\n"));
    }

    #[test]
    fn detects_cfg_attr_no_std() {
        // hashbrown 0.17 / foldhash use the conditional form. Both
        // produce `no_std` builds in normal usage.
        assert!(scan_for_no_std(
            "#![cfg_attr(not(doc), no_std)]\npub fn x() {}\n"
        ));
        assert!(scan_for_no_std(
            "#![cfg_attr(all(not(test), not(feature = \"std\")), no_std)]\npub fn x() {}\n"
        ));
    }

    #[test]
    fn detects_multiline_cfg_attr() {
        // Some crates wrap long cfg_attr expressions across lines.
        assert!(scan_for_no_std(
            "#![cfg_attr(\n    all(not(test), not(feature = \"std\")),\n    no_std\n)]\npub fn x() {}\n"
        ));
    }

    #[test]
    fn rejects_no_std_substring() {
        // `not_no_std` and `no_std_foo` aren't the `no_std` attribute.
        assert!(!scan_for_no_std("#![allow(not_no_std)]\npub fn x() {}\n"));
        assert!(!scan_for_no_std("#![allow(no_std_foo)]\npub fn x() {}\n"));
    }

    #[test]
    fn libstd_artifact_recognizes_unix() {
        assert!(is_libstd_artifact("libstd-6eb430e61c7aea8a.dylib"));
        assert!(is_libstd_artifact("libstd-6eb430e61c7aea8a.so"));
        assert!(is_libstd_artifact("libstd-6eb430e61c7aea8a.rlib"));
        assert!(is_libstd_artifact("libstd-6eb430e61c7aea8a.rmeta"));
    }

    #[test]
    fn libstd_artifact_recognizes_windows() {
        assert!(is_libstd_artifact("std-6eb430e61c7aea8a.dll"));
    }

    #[test]
    fn libstd_artifact_rejects_neighbors() {
        // `libstd_detect`, `libstdc++`, etc. share the prefix but
        // aren't libstd.
        assert!(!is_libstd_artifact("libstd_detect-abc.rlib"));
        assert!(!is_libstd_artifact("libstdc++.dylib"));
        assert!(!is_libstd_artifact("libstd.dylib"));
    }
}
