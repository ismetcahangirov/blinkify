//! Shared by the engine's integration tests.
//!
//! These tests run the real bundled sidecar, because the behaviour under test
//! is what FFmpeg actually does with a file. They fail — never skip — when it
//! is absent: a test that quietly passes on a machine without the thing it
//! tests is not a test. `pnpm sidecar:fetch` puts it in place.

#![allow(dead_code, clippy::expect_used, unreachable_pub)]

use std::path::PathBuf;

use blinkify_engine::Sidecar;

/// The sidecar as `pnpm sidecar:fetch` leaves it in the source tree, or the
/// directory named by `BLINKIFY_SIDECAR_DIR`.
pub fn sidecar() -> Sidecar {
    let dir = std::env::var_os("BLINKIFY_SIDECAR_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src-tauri/binaries"),
        PathBuf::from,
    );
    Sidecar::in_dir(&dir).expect("the FFmpeg sidecar is missing; run `pnpm sidecar:fetch`")
}
