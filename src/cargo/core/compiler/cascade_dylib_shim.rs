//! Per-build helpers for the unstable `cascade-dylib` feature
//! (`-Z cascade-dylib[=spec,...]`).
//!
//! At present, this module exposes the `@loader_path/deps` rpath
//! injection that every cascade dylib (root or promoted) needs so the
//! dynamic loader can find SVH-stamped sibling dylibs at
//! `target/<profile>/deps/`. The `no_std` std-force-link plumbing —
//! the other half of the cascade-dylib machinery — lives in this same
//! file and is added in a follow-up commit; this commit lands only the
//! rpath piece so the cascade feature itself can be reviewed
//! independently of the runtime-symbol hookup for `no_std` promoted
//! crates.

use std::ffi::OsString;

use crate::core::compiler::{BuildContext, CrateType, Unit};
use crate::util::CargoResult;

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
