//! Tests for profile overrides (build-override and per-package overrides).

use crate::prelude::*;
use cargo_test_support::registry::Package;
use cargo_test_support::{basic_lib_manifest, basic_manifest, project, str};

#[cargo_test]
fn profile_override_basic() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"
                authors = []

                [dependencies]
                bar = {path = "bar"}

                [profile.dev]
                opt-level = 1

                [profile.dev.package.bar]
                opt-level = 3
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "")
        .build();

    p.cargo("check -v")
        .with_stderr_data(str![[r#"
[LOCKING] 1 package to latest compatible version
[CHECKING] bar v0.5.0 ([ROOT]/foo/bar)
[RUNNING] `rustc --crate-name bar [..] -C opt-level=3 [..]`
[CHECKING] foo v0.0.1 ([ROOT]/foo)
[RUNNING] `rustc --crate-name foo [..] -C opt-level=1 [..]`
[FINISHED] `dev` profile [optimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

#[cargo_test]
fn profile_override_warnings() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"

                [dependencies]
                bar = {path = "bar"}

                [profile.dev.package.bart]
                opt-level = 3

                [profile.dev.package.no-suggestion]
                opt-level = 3

                [profile.dev.package."bar:1.2.3"]
                opt-level = 3
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "")
        .build();

    p.cargo("build").with_stderr_data(str![[r#"
...
[WARNING] profile package spec `bar@1.2.3` in profile `dev` has a version or URL that does not match any of the packages: bar v0.5.0 ([ROOT]/foo/bar)
[WARNING] profile package spec `bart` in profile `dev` did not match any packages

[HELP] a package with a similar name exists: `bar`
[WARNING] profile package spec `no-suggestion` in profile `dev` did not match any packages
[COMPILING] bar v0.5.0 ([ROOT]/foo/bar)
[COMPILING] foo v0.0.1 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]]).run();
}

#[cargo_test]
fn profile_override_bad_settings() {
    let bad_values = [
        (
            "panic = \"abort\"",
            "`panic` may not be specified in a `package` profile",
        ),
        (
            "lto = true",
            "`lto` may not be specified in a `package` profile",
        ),
        (
            "rpath = true",
            "`rpath` may not be specified in a `package` profile",
        ),
        ("package = {}", "package-specific profiles cannot be nested"),
    ];
    for &(snippet, expected) in bad_values.iter() {
        let p = project()
            .file(
                "Cargo.toml",
                &format!(
                    r#"
                        [package]
                        name = "foo"
                        version = "0.0.1"
                        edition = "2015"

                        [dependencies]
                        bar = {{path = "bar"}}

                        [profile.dev.package.bar]
                        {}
                    "#,
                    snippet
                ),
            )
            .file("src/lib.rs", "")
            .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
            .file("bar/src/lib.rs", "")
            .build();

        p.cargo("check")
            .with_status(101)
            .with_stderr_data(format!(
                "\
...
Caused by:\n  {}
",
                expected
            ))
            .run();
    }
}

#[cargo_test]
fn profile_override_hierarchy() {
    // Test that the precedence rules are correct for different types.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
            [workspace]
            members = ["m1", "m2", "m3"]

            [profile.dev]
            codegen-units = 1

            [profile.dev.package.m2]
            codegen-units = 2

            [profile.dev.package."*"]
            codegen-units = 3

            [profile.dev.build-override]
            codegen-units = 4
            "#,
        )
        // m1
        .file(
            "m1/Cargo.toml",
            r#"
            [package]
            name = "m1"
            version = "0.0.1"
            edition = "2015"

            [dependencies]
            m2 = { path = "../m2" }
            dep = { path = "../../dep" }
            "#,
        )
        .file("m1/src/lib.rs", "extern crate m2; extern crate dep;")
        .file("m1/build.rs", "fn main() {}")
        // m2
        .file(
            "m2/Cargo.toml",
            r#"
            [package]
            name = "m2"
            version = "0.0.1"
            edition = "2015"

            [dependencies]
            m3 = { path = "../m3" }

            [build-dependencies]
            m3 = { path = "../m3" }
            dep = { path = "../../dep" }
            "#,
        )
        .file("m2/src/lib.rs", "extern crate m3;")
        .file(
            "m2/build.rs",
            "extern crate m3; extern crate dep; fn main() {}",
        )
        // m3
        .file("m3/Cargo.toml", &basic_lib_manifest("m3"))
        .file("m3/src/lib.rs", "")
        .build();

    // dep (outside of workspace)
    let _dep = project()
        .at("dep")
        .file("Cargo.toml", &basic_lib_manifest("dep"))
        .file("src/lib.rs", "")
        .build();

    // Profiles should be:
    // m3: 4 (as build.rs dependency)
    // m3: 1 (as [profile.dev] as workspace member)
    // dep: 3 (as [profile.dev.package."*"] as non-workspace member)
    // m1 build.rs: 4 (as [profile.dev.build-override])
    // m2 build.rs: 2 (as [profile.dev.package.m2])
    // m2: 2 (as [profile.dev.package.m2])
    // m1: 1 (as [profile.dev])

    p.cargo("build -v")
        .with_stderr_data(str![[r#"
[LOCKING] 1 package to latest compatible version
[COMPILING] m3 v0.5.0 ([ROOT]/foo/m3)
[COMPILING] dep v0.5.0 ([ROOT]/dep)
[RUNNING] `rustc --crate-name m3 --edition=2015 m3/src/lib.rs [..] --crate-type lib --emit=[..]link[..]-C codegen-units=4 [..]`
[RUNNING] `rustc --crate-name dep [..][ROOT]/dep/src/lib.rs [..] --crate-type lib --emit=[..]link[..]-C codegen-units=3 [..]`
[RUNNING] `rustc --crate-name m3 --edition=2015 m3/src/lib.rs [..] --crate-type lib --emit=[..]link[..]-C codegen-units=1 [..]`
[RUNNING] `rustc --crate-name build_script_build --edition=2015 m1/build.rs [..] --crate-type bin --emit=[..]link[..]-C codegen-units=4 [..]`
[COMPILING] m2 v0.0.1 ([ROOT]/foo/m2)
[RUNNING] `rustc --crate-name build_script_build --edition=2015 m2/build.rs [..] --crate-type bin --emit=[..]link[..]-C codegen-units=2 [..]`
[RUNNING] `[ROOT]/foo/target/debug/build/m1-[HASH]/build-script-build`
[RUNNING] `[ROOT]/foo/target/debug/build/m2-[HASH]/build-script-build`
[RUNNING] `rustc --crate-name m2 --edition=2015 m2/src/lib.rs [..] --crate-type lib --emit=[..]link[..]-C codegen-units=2 [..]`
[COMPILING] m1 v0.0.1 ([ROOT]/foo/m1)
[RUNNING] `rustc --crate-name m1 --edition=2015 m1/src/lib.rs [..] --crate-type lib --emit=[..]link[..]-C codegen-units=1 [..]`
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]].unordered())
        .run();
}

#[cargo_test]
fn profile_override_spec_multiple() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
            [package]
            name = "foo"
            version = "0.0.1"
            edition = "2015"

            [dependencies]
            bar = { path = "bar" }

            [profile.dev.package.bar]
            opt-level = 3

            [profile.dev.package."bar:0.5.0"]
            opt-level = 3
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "")
        .build();

    p.cargo("check -v")
        .with_status(101)
        .with_stderr_data(str![[r#"
...
[ERROR] multiple package overrides in profile `dev` match package `bar v0.5.0 ([ROOT]/foo/bar)`
found package specs: bar, bar@0.5.0

"#]])
        .run();
}

#[cargo_test]
fn profile_override_spec_with_version() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
            [package]
            name = "foo"
            version = "0.0.1"
            edition = "2015"

            [dependencies]
            bar = { path = "bar" }

            [profile.dev.package."bar:0.5.0"]
            codegen-units = 2
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "")
        .build();

    p.cargo("check -v")
        .with_stderr_data(str![[r#"
...
[CHECKING] bar v0.5.0 ([ROOT]/foo/bar)
[RUNNING] `rustc [..]bar/src/lib.rs [..] -C codegen-units=2 [..]`
...
"#]])
        .run();
}

#[cargo_test]
fn profile_override_spec_with_partial_version() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
            [package]
            name = "foo"
            version = "0.0.1"
            edition = "2015"

            [dependencies]
            bar = { path = "bar" }

            [profile.dev.package."bar:0.5"]
            codegen-units = 2
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "")
        .build();

    p.cargo("check -v")
        .with_stderr_data(str![[r#"
...
[CHECKING] bar v0.5.0 ([ROOT]/foo/bar)
[RUNNING] `rustc [..]bar/src/lib.rs [..] -C codegen-units=2 [..]`
...
"#]])
        .run();
}

#[cargo_test]
fn profile_override_spec() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
            [workspace]
            members = ["m1", "m2"]

            [profile.dev.package."dep:1.0.0"]
            codegen-units = 1

            [profile.dev.package."dep:2.0.0"]
            codegen-units = 2
            "#,
        )
        // m1
        .file(
            "m1/Cargo.toml",
            r#"
            [package]
            name = "m1"
            version = "0.0.1"
            edition = "2015"

            [dependencies]
            dep = { path = "../../dep1" }
            "#,
        )
        .file("m1/src/lib.rs", "extern crate dep;")
        // m2
        .file(
            "m2/Cargo.toml",
            r#"
            [package]
            name = "m2"
            version = "0.0.1"
            edition = "2015"

            [dependencies]
            dep = {path = "../../dep2" }
            "#,
        )
        .file("m2/src/lib.rs", "extern crate dep;")
        .build();

    project()
        .at("dep1")
        .file("Cargo.toml", &basic_manifest("dep", "1.0.0"))
        .file("src/lib.rs", "")
        .build();

    project()
        .at("dep2")
        .file("Cargo.toml", &basic_manifest("dep", "2.0.0"))
        .file("src/lib.rs", "")
        .build();

    p.cargo("check -v")
        .with_stderr_data(
            str![[r#"
...
[RUNNING] `rustc [..][ROOT]/dep1/src/lib.rs [..] -C codegen-units=1 [..]`
[RUNNING] `rustc [..][ROOT]/dep2/src/lib.rs [..] -C codegen-units=2 [..]`
...
"#]]
            .unordered(),
        )
        .run();
}

#[cargo_test]
fn override_proc_macro() {
    Package::new("shared", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
            [package]
            name = "foo"
            version = "0.1.0"
            edition = "2018"

            [dependencies]
            shared = "1.0"
            pm = {path = "pm"}

            [profile.dev.build-override]
            codegen-units = 4
            "#,
        )
        .file("src/lib.rs", r#"pm::eat!{}"#)
        .file(
            "pm/Cargo.toml",
            r#"
            [package]
            name = "pm"
            version = "0.1.0"
            edition = "2015"

            [lib]
            proc-macro = true

            [dependencies]
            shared = "1.0"
            "#,
        )
        .file(
            "pm/src/lib.rs",
            r#"
            extern crate proc_macro;
            use proc_macro::TokenStream;

            #[proc_macro]
            pub fn eat(_item: TokenStream) -> TokenStream {
                "".parse().unwrap()
            }
            "#,
        )
        .build();

    p.cargo("check -v")
        // Shared built for the proc-macro.
        .with_stderr_data(str![[r#"
...
[RUNNING] `rustc [..]--crate-name shared [..] -C codegen-units=4[..]`
...
[RUNNING] `rustc [..]--crate-name pm [..] -C codegen-units=4[..]`
...
"#]])
        // Shared built for the library.
        .with_stderr_line_without(
            &["[RUNNING] `rustc --crate-name shared --edition=2015"],
            &["-C codegen-units"],
        )
        .with_stderr_line_without(
            &["[RUNNING] `rustc [..]--crate-name foo"],
            &["-C codegen-units"],
        )
        .run();
}

#[cargo_test]
fn no_warning_ws() {
    // https://github.com/rust-lang/cargo/issues/7378, avoid warnings in a workspace.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
            [workspace]
            members = ["a", "b"]

            [profile.dev.package.a]
            codegen-units = 3
            "#,
        )
        .file("a/Cargo.toml", &basic_manifest("a", "0.1.0"))
        .file("a/src/lib.rs", "")
        .file("b/Cargo.toml", &basic_manifest("b", "0.1.0"))
        .file("b/src/lib.rs", "")
        .build();

    p.cargo("check -p b")
        .with_stderr_data(str![[r#"
[CHECKING] b v0.1.0 ([ROOT]/foo/b)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

#[cargo_test]
fn build_override_shared() {
    // A dependency with a build script that is shared with a build
    // dependency, using different profile settings. That is:
    //
    // foo DEBUG=2
    // ├── common DEBUG=2
    // │   └── common Run build.rs DEBUG=2
    // │       └── common build.rs DEBUG=0 (build_override)
    // └── foo Run build.rs DEBUG=2
    //     └── foo build.rs DEBUG=0 (build_override)
    //         └── common DEBUG=0 (build_override)
    //             └── common Run build.rs DEBUG=0 (build_override)
    //                 └── common build.rs DEBUG=0 (build_override)
    //
    // The key part here is that `common` RunCustomBuild is run twice, once
    // with DEBUG=2 (as a dependency of foo) and once with DEBUG=0 (as a
    // build-dependency of foo's build script).
    Package::new("common", "1.0.0")
        .file(
            "build.rs",
            r#"
            fn main() {
                if std::env::var("DEBUG").unwrap() != "false" {
                    println!("cargo::rustc-cfg=foo_debug");
                } else {
                    println!("cargo::rustc-cfg=foo_release");
                }
            }
            "#,
        )
        .file(
            "src/lib.rs",
            r#"
            pub fn foo() -> u32 {
                if cfg!(foo_debug) {
                    assert!(cfg!(debug_assertions));
                    1
                } else if cfg!(foo_release) {
                    assert!(!cfg!(debug_assertions));
                    2
                } else {
                    panic!("not set");
                }
            }
            "#,
        )
        .publish();

    let p = project()
        .file(
            "Cargo.toml",
            r#"
            [package]
            name = "foo"
            version = "0.1.0"
            edition = "2018"

            [build-dependencies]
            common = "1.0"

            [dependencies]
            common = "1.0"

            [profile.dev.build-override]
            debug = 0
            debug-assertions = false
            "#,
        )
        .file(
            "build.rs",
            r#"
            fn main() {
                assert_eq!(common::foo(), 2);
            }
            "#,
        )
        .file(
            "src/main.rs",
            r#"
            fn main() {
                assert_eq!(common::foo(), 1);
            }
            "#,
        )
        .build();

    p.cargo("run").run();
}

#[cargo_test]
fn cascade_dylib_cli_root_promotes_named_package() {
    // `-Z cascade-dylib=bar` makes `bar` a cascade root. With no further
    // runtime deps, that just promotes `bar` itself to `lib + dylib`.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "")
        .build();

    p.cargo("build -v -Z cascade-dylib=bar")
        .masquerade_as_nightly_cargo(&[])
        .with_stderr_data(str![[r#"
[LOCKING] 1 package to latest compatible version
     Cascade promoted 1 package(s) to dylib in profile `dev`
[COMPILING] bar v0.5.0 ([ROOT]/foo/bar)
[RUNNING] `rustc --crate-name bar [..]--crate-type lib --crate-type dylib [..]`
[COMPILING] foo v0.0.1 ([ROOT]/foo)
[RUNNING] `rustc --crate-name foo [..]`
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

#[cargo_test]
fn cascade_dylib_cli_unknown_spec_warns() {
    // A `-Z cascade-dylib=<spec>` entry that doesn't match any package in
    // the resolve graph emits a warning but does not fail the build.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "")
        .build();

    p.cargo("check -Z cascade-dylib=does-not-exist")
        .masquerade_as_nightly_cargo(&[])
        .with_stderr_contains(
            "[WARNING] -Z cascade-dylib spec `does-not-exist` did not match any package in the resolve graph",
        )
        .run();
}

#[cargo_test]
fn cascade_dylib_no_std_std_link() {
    // A `no_std` crate cascade-promoted to `dylib` would normally fail to
    // link (no `#[panic_handler]`, no `#[global_allocator]`, no unwinder).
    // Cargo force-links `std` itself by passing
    //   `--extern force:std=<libstd-*.dylib>`
    //   `--extern force:std=<libstd-*.rmeta>`
    // so the dylib link step succeeds. Routing the metadata edge through
    // the real `std` toolchain crate (which is available in both rlib and
    // dylib formats) sidesteps rustc's link-format uniformity invariant
    // for downstream consumers that reach the promoted dylib via mixed
    // rlib/dylib paths.
    //
    // We assert on the rustc invocation rather than on build success
    // because `--extern force:` requires `-Z unstable-options`, which
    // only nightly rustc accepts. The cargo behavior under test is
    // independent of which rustc actually runs.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2018"

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file(
            "bar/src/lib.rs",
            "#![no_std]\npub fn answer() -> u32 { 42 }\n",
        )
        .build();

    p.cargo("build -v -Z cascade-dylib=bar")
        .masquerade_as_nightly_cargo(&[])
        .with_stderr_contains("[..]--crate-type lib --crate-type dylib [..]")
        .with_stderr_contains("[..]--extern [..]force:std=[..]libstd-[..]")
        .with_stderr_contains("[..]-Z unstable-options[..]")
        .without_status()
        .run();
}

#[cargo_test]
fn cascade_dylib_no_std_std_link_block_doc_comment() {
    // Some crates (e.g. pin-project-lite) lead their `lib.rs` with a
    // `/*! ... */` outer block doc-comment before `#![no_std]`. The
    // textual scan must skip block comments — including multi-line —
    // when looking for the attribute, otherwise std-injection misses
    // the crate and the dylib link fails on `#[panic_handler]`.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2018"

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file(
            "bar/src/lib.rs",
            "// SPDX-License-Identifier: Apache-2.0\n\
             \n\
             /*!\n\
             A lightweight crate.\n\
             \n\
             Multi-line doc that spans several lines before the\n\
             attribute.\n\
             */\n\
             \n\
             #![no_std]\n\
             pub fn answer() -> u32 { 42 }\n",
        )
        .build();

    p.cargo("build -v -Z cascade-dylib=bar")
        .masquerade_as_nightly_cargo(&[])
        .with_stderr_contains("[..]--extern [..]force:std=[..]libstd-[..]")
        .without_status()
        .run();
}

#[cargo_test]
fn cascade_dylib_no_std_std_link_cfg_attr() {
    // Real-world `no_std` crates often use the conditional form
    // (e.g. `#![cfg_attr(not(test), no_std)]`). The detection
    // heuristic recognizes both shapes.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2018"

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file(
            "bar/src/lib.rs",
            "#![cfg_attr(not(test), no_std)]\npub fn answer() -> u32 { 42 }\n",
        )
        .build();

    p.cargo("build -v -Z cascade-dylib=bar")
        .masquerade_as_nightly_cargo(&[])
        .with_stderr_contains("[..]--extern [..]force:std=[..]libstd-[..]")
        .without_status()
        .run();
}

#[cargo_test]
fn cascade_dylib_std_link_skipped_for_std_crate() {
    // A regular (non-`no_std`) crate promoted to `dylib` does not need
    // the explicit `std` force-link — `extern crate std` is implicit and
    // pulls in all the runtime symbols itself. The `--extern force:std`
    // flag must be absent from rustc's invocation in that case.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2018"

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "pub fn answer() -> u32 { 42 }\n")
        .build();

    p.cargo("build -v -Z cascade-dylib=bar")
        .masquerade_as_nightly_cargo(&[])
        .with_stderr_data(str![[r#"
[LOCKING] 1 package to latest compatible version
     Cascade promoted 1 package(s) to dylib in profile `dev`
[COMPILING] bar v0.5.0 ([ROOT]/foo/bar)
[RUNNING] `rustc --crate-name bar [..]--crate-type lib --crate-type dylib [..]`
[COMPILING] foo v0.0.1 ([ROOT]/foo)
[RUNNING] `rustc --crate-name foo [..]`
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .with_stderr_does_not_contain("force:std=")
        .run();
}

#[cargo_test]
fn cascade_dylib_std_link_passes_dylib_and_rmeta() {
    // The shipped standard library uses `-Zembed-metadata=no`: the
    // `.dylib` carries only a metadata stub and rustc demands the
    // `.rmeta` separately. Cargo passes both `--extern` entries so the
    // dylib gets dynamic linkage AND full metadata.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2018"

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file(
            "bar/src/lib.rs",
            "#![no_std]\npub fn answer() -> u32 { 42 }\n",
        )
        .build();

    // Two `--extern force:std=` entries on bar's invocation: one for the
    // dylib (gives linkage), one for the rmeta (gives metadata).
    p.cargo("build -v -Z cascade-dylib=bar")
        .masquerade_as_nightly_cargo(&[])
        .with_stderr_contains("[..]--crate-name bar [..]--extern [..]force:std=[..]libstd-[..]")
        .with_stderr_contains("[..]--extern [..]force:std=[..]libstd-[..].rmeta[..]")
        .without_status()
        .run();
}

#[cargo_test]
fn cascade_dylib_implicit_from_manifest() {
    // A crate whose own `[lib].crate-type` contains `dylib` is automatically
    // a cascade root once `-Z cascade-dylib` is on (bare flag, no spec
    // list): every non-proc-macro package reachable from it via runtime-dep
    // edges is promoted to `["lib", "dylib"]` for the active profile. This
    // is the canonical hot-reload-style flow — no manifest mutation
    // required beyond the existing `crate-type` declaration the workspace
    // lib already had.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"

                [lib]
                crate-type = ["lib", "dylib"]

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "extern crate bar;")
        .file(
            "bar/Cargo.toml",
            r#"
                [package]
                name = "bar"
                version = "0.5.0"
                edition = "2015"

                [dependencies]
                baz = { path = "../baz" }
            "#,
        )
        .file("bar/src/lib.rs", "extern crate baz;")
        .file("baz/Cargo.toml", &basic_lib_manifest("baz"))
        .file("baz/src/lib.rs", "")
        .build();

    p.cargo("build -v -Z cascade-dylib")
        .masquerade_as_nightly_cargo(&[])
        // foo (manifest dylib) is the root; bar and baz are reached via
        // runtime-dep BFS and synthesized into per-package overrides.
        .with_stderr_data(str![[r#"
[LOCKING] 2 packages to latest compatible versions
     Cascade promoted 2 package(s) to dylib in profile `dev`
[COMPILING] baz v0.5.0 ([ROOT]/foo/baz)
[RUNNING] `rustc --crate-name baz [..]--crate-type lib --crate-type dylib [..]`
[COMPILING] bar v0.5.0 ([ROOT]/foo/bar)
[RUNNING] `rustc --crate-name bar [..]--crate-type lib --crate-type dylib [..]`
[COMPILING] foo v0.0.1 ([ROOT]/foo)
[RUNNING] `rustc --crate-name foo [..]--crate-type lib --crate-type dylib [..]`
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

#[cargo_test]
#[cfg(unix)]
fn cascade_dylib_emits_loader_path_deps_rpath() {
    // Every dylib (root or cascade-promoted) under `-Z cascade-dylib` gets
    // an extra `-Wl,-rpath,@loader_path/deps` (`$ORIGIN/deps` on Linux)
    // so the dynamic loader finds the SVH-stamped sibling dylibs at
    // `target/<profile>/deps/` regardless of who's loading the dylib.
    // Without this, consumers that don't have `DYLD_FALLBACK_LIBRARY_PATH`
    // set (`cargo install`'d binaries, `dlopen` from arbitrary tooling)
    // hit `dlopen failed` on the first `@rpath/lib*-<svh>.dylib`
    // reference.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"

                [lib]
                crate-type = ["lib", "dylib"]

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "extern crate bar;")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "")
        .build();

    // `[..]/deps` matches both `@loader_path/deps` (macOS) and
    // `$ORIGIN/deps` (Linux + BSDs). The root foo dylib AND the
    // cascade-promoted bar dylib both carry the rpath.
    p.cargo("build -v -Z cascade-dylib")
        .masquerade_as_nightly_cargo(&[])
        .with_stderr_data(str![[r#"
[LOCKING] 1 package to latest compatible version
     Cascade promoted 1 package(s) to dylib in profile `dev`
[COMPILING] bar v0.5.0 ([ROOT]/foo/bar)
[RUNNING] `rustc --crate-name bar [..]link-arg=-Wl,-rpath,[..]/deps[..]`
[COMPILING] foo v0.0.1 ([ROOT]/foo)
[RUNNING] `rustc --crate-name foo [..]link-arg=-Wl,-rpath,[..]/deps[..]`
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

#[cargo_test]
fn cascade_dylib_cli_spec_walks_runtime_closure() {
    // A `-Z cascade-dylib=bar` entry makes `bar` a cascade root, walking
    // through `bar`'s runtime deps. `baz`, reached only through `bar`'s
    // `[dependencies]`, is promoted by the cascade BFS.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "extern crate bar;")
        .file(
            "bar/Cargo.toml",
            r#"
                [package]
                name = "bar"
                version = "0.5.0"
                edition = "2015"

                [dependencies]
                baz = { path = "../baz" }
            "#,
        )
        .file("bar/src/lib.rs", "extern crate baz;")
        .file("baz/Cargo.toml", &basic_lib_manifest("baz"))
        .file("baz/src/lib.rs", "")
        .build();

    p.cargo("build -v -Z cascade-dylib=bar")
        .masquerade_as_nightly_cargo(&[])
        // bar is the named root; baz is promoted by the cascade through
        // bar's runtime dep.
        .with_stderr_data(str![[r#"
...
[RUNNING] `rustc --crate-name baz [..]--crate-type lib --crate-type dylib [..]`
...
[RUNNING] `rustc --crate-name bar [..]--crate-type lib --crate-type dylib [..]`
...
"#]])
        .run();
}

#[cargo_test]
fn cascade_dylib_no_op_without_flag() {
    // The cascade only fires when `-Z cascade-dylib` is passed. A package
    // using `crate-type = ["lib", "dylib"]` in its manifest without the
    // flag gets cargo's standard dylib emission for itself but no cascade
    // through deps — i.e. the same behavior as stable cargo.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"

                [lib]
                crate-type = ["lib", "dylib"]

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "extern crate bar;")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "")
        .build();

    p.cargo("build -v")
        // bar must NOT be cascade-promoted: built as plain rlib only.
        .with_stderr_line_without(
            &["[RUNNING] `rustc --crate-name bar"],
            &["--crate-type dylib"],
        )
        .run();
}

#[cargo_test]
fn cascade_dylib_skips_proc_macro() {
    // Proc-macro crates can't be dylibs. When a CLI-named root is a
    // proc-macro, cascade root collection skips it; the intern site
    // also filters it out defensively, so the proc-macro keeps its
    // native kind.
    Package::new("shared", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.1.0"
                edition = "2018"

                [dependencies]
                pm = { path = "pm" }
            "#,
        )
        .file("src/lib.rs", r#"pm::eat!{}"#)
        .file(
            "pm/Cargo.toml",
            r#"
                [package]
                name = "pm"
                version = "0.1.0"
                edition = "2015"

                [lib]
                proc-macro = true

                [dependencies]
                shared = "1.0"
            "#,
        )
        .file(
            "pm/src/lib.rs",
            r#"
                extern crate proc_macro;
                use proc_macro::TokenStream;
                #[proc_macro]
                pub fn eat(_item: TokenStream) -> TokenStream {
                    "".parse().unwrap()
                }
            "#,
        )
        .build();

    p.cargo("build -v -Z cascade-dylib=pm")
        .masquerade_as_nightly_cargo(&[])
        // pm is built with --crate-type proc-macro, NOT promoted.
        .with_stderr_line_without(
            &["[RUNNING] `rustc --crate-name pm"],
            &["--crate-type lib --crate-type dylib"],
        )
        .run();
}

#[cargo_test]
fn cascade_dylib_skips_build_dep() {
    // Build-script dep edges must not be followed: build scripts run on the
    // host and their deps don't reach the runtime dylib graph.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "extern crate bar;")
        .file(
            "bar/Cargo.toml",
            r#"
                [package]
                name = "bar"
                version = "0.5.0"
                edition = "2015"
                build = "build.rs"

                [build-dependencies]
                baz = { path = "../baz" }
            "#,
        )
        .file("bar/src/lib.rs", "")
        .file("bar/build.rs", "fn main() {}")
        .file("baz/Cargo.toml", &basic_lib_manifest("baz"))
        .file("baz/src/lib.rs", "")
        .build();

    p.cargo("build -v -Z cascade-dylib=bar")
        .masquerade_as_nightly_cargo(&[])
        // bar is the CLI-named cascade root and IS promoted.
        .with_stderr_data(str![[r#"
...
[RUNNING] `rustc --crate-name bar [..]--crate-type lib --crate-type dylib [..]`
...
"#]])
        // baz is reached only via a build-dep edge and must NOT be promoted.
        .with_stderr_line_without(
            &["[RUNNING] `rustc --crate-name baz"],
            &["--crate-type dylib"],
        )
        .run();
}

#[cargo_test]
fn cascade_dylib_skips_release_profile_for_manifest_root() {
    // Source 1 (manifest `[lib].crate-type` includes `dylib`) is profile-
    // agnostic in a Cargo manifest, but cascade is a dev-loop concept.
    // Building under `release` (or any profile not transitively inheriting
    // `dev`) must NOT treat the manifest dylib as a cascade root, so deps
    // stay rlib-only. This avoids release-only link failures on crates
    // whose Rust wrapper statically links a C archive (tree-sitter et al.).
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"

                [lib]
                crate-type = ["lib", "dylib"]

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "extern crate bar;")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "")
        .build();

    p.cargo("build -v --release -Z cascade-dylib")
        .masquerade_as_nightly_cargo(&[])
        // bar must NOT be cascade-promoted in release: rlib only.
        .with_stderr_line_without(
            &["[RUNNING] `rustc --crate-name bar"],
            &["--crate-type dylib"],
        )
        .run();
}

#[cargo_test]
fn cascade_dylib_release_cli_root_still_fires() {
    // Power-user opt-in: source 1 (manifest dylib) doesn't contribute a
    // cascade root in `release`, but a CLI-named root does. The cascade
    // BFS walks from the named root through its runtime deps as usual.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "extern crate bar;")
        .file(
            "bar/Cargo.toml",
            r#"
                [package]
                name = "bar"
                version = "0.5.0"
                edition = "2015"

                [dependencies]
                baz = { path = "../baz" }
            "#,
        )
        .file("bar/src/lib.rs", "extern crate baz;")
        .file("baz/Cargo.toml", &basic_lib_manifest("baz"))
        .file("baz/src/lib.rs", "")
        .build();

    p.cargo("build -v --release -Z cascade-dylib=bar")
        .masquerade_as_nightly_cargo(&[])
        .with_stderr_data(str![[r#"
...
[RUNNING] `rustc --crate-name baz [..]--crate-type lib --crate-type dylib [..]`
...
[RUNNING] `rustc --crate-name bar [..]--crate-type lib --crate-type dylib [..]`
...
"#]])
        .run();
}

#[cargo_test]
fn cascade_dylib_fires_in_test_profile() {
    // The `test` profile inherits `dev` (ProfileRoot::Debug), so source 1
    // still contributes a cascade root under `cargo test`. This keeps the
    // cross-dylib hot-reload integration test workflow working.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"

                [lib]
                crate-type = ["lib", "dylib"]

                [dependencies]
                bar = { path = "bar" }
            "#,
        )
        .file("src/lib.rs", "extern crate bar;")
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "")
        .build();

    p.cargo("test -v --no-run -Z cascade-dylib")
        .masquerade_as_nightly_cargo(&[])
        .with_stderr_data(str![[r#"
...
[RUNNING] `rustc --crate-name bar [..]--crate-type lib --crate-type dylib [..]`
...
"#]])
        .run();
}
