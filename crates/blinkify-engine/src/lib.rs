//! The Blinkify media engine.
//!
//! # The one rule this crate exists to hold
//!
//! `CLAUDE.md` section 1: no operation may degrade media quality that did not
//! have to be degraded. Every export decision resolves to exactly one of three
//! tiers, and the planner must choose the **lowest tier that satisfies the
//! edit**.
//!
//! # No Tauri
//!
//! This crate must not depend on `tauri`, directly or transitively. An engine
//! crate that needs a window cannot be tested without one, which means it will
//! stop being tested. `cargo test -p blinkify-engine` runs with no window, no
//! `WebView2` and no renderer, and CI asserts the dependency is absent.

pub mod audio;
pub mod cache;
pub mod capability;
pub mod capability_cache;
pub mod decode;
pub mod display_driver;
pub mod export;
pub mod filmstrip;
pub mod keyframes;
pub mod orchestrator;
pub mod playback;
pub mod probe;
pub mod project;
pub mod proxy;
pub mod seek;
pub mod sidecar;
pub mod tier;
pub mod time;
pub mod waveform;

pub use capability::{EncoderCapabilities, VideoCodec};
pub use sidecar::{Sidecar, SidecarError};
pub use tier::{ExportTier, ReEncodeReason, SeamReason};
