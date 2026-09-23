//! Locating the bundled FFmpeg sidecar.
//!
//! `ADR-0002`: Blinkify runs its own LGPL build of `ffmpeg.exe` and
//! `ffprobe.exe` as separate processes. It never links FFmpeg, and it never
//! runs whatever FFmpeg happens to be on the user's `PATH` — that one may be a
//! GPL build, may be a different version with different timestamp behaviour,
//! and may not exist. Silently falling back to it would make both the licence
//! position and the behaviour unknowable.
//!
//! So the sidecar is only ever named by an absolute path in a directory the
//! caller chose, and a missing binary is an error rather than a search.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// The Rust target triple Tauri appends to an `externalBin` in the source
/// tree. The bundler strips it when it installs the binary next to the
/// application, so both spellings are accepted.
pub const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";

/// The sidecar could not be found where it must be.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SidecarError {
    /// A binary is absent from the directory. The installation is damaged, or
    /// in development `pnpm sidecar:fetch` has not been run.
    #[error(
        "the bundled {name} was not found in {dir}; the installation is incomplete (in development, run `pnpm sidecar:fetch`)"
    )]
    Missing { name: &'static str, dir: PathBuf },
    /// The running executable's own location could not be determined.
    #[error("could not determine where Blinkify is installed: {0}")]
    NoExecutableDirectory(String),
}

/// The absolute paths of the two sidecar binaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sidecar {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

impl Sidecar {
    /// The sidecar in `dir`, as installed (`ffmpeg.exe`) or as it sits in the
    /// source tree (`ffmpeg-x86_64-pc-windows-msvc.exe`).
    ///
    /// # Errors
    ///
    /// [`SidecarError::Missing`] if either binary is absent. There is no
    /// fallback, by design.
    pub fn in_dir(dir: &Path) -> Result<Self, SidecarError> {
        Ok(Self {
            ffmpeg: find(dir, "ffmpeg")?,
            ffprobe: find(dir, "ffprobe")?,
        })
    }

    /// The sidecar installed next to the running executable, which is where
    /// the Tauri bundler puts every `externalBin` — and, under `tauri dev`,
    /// where it copies them.
    ///
    /// # Errors
    ///
    /// As [`Sidecar::in_dir`], or if the executable's location is unknown.
    pub fn beside_current_exe() -> Result<Self, SidecarError> {
        let exe = std::env::current_exe()
            .map_err(|error| SidecarError::NoExecutableDirectory(error.to_string()))?;
        let dir = exe.parent().ok_or_else(|| {
            SidecarError::NoExecutableDirectory(format!("{} has no parent", exe.display()))
        })?;
        Self::in_dir(dir)
    }

    /// `ffmpeg.exe`, as an absolute path.
    #[must_use]
    pub fn ffmpeg(&self) -> &Path {
        &self.ffmpeg
    }

    /// `ffprobe.exe`, as an absolute path.
    #[must_use]
    pub fn ffprobe(&self) -> &Path {
        &self.ffprobe
    }
}

fn find(dir: &Path, name: &'static str) -> Result<PathBuf, SidecarError> {
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    [
        dir.join(format!("{name}.exe")),
        dir.join(format!("{name}-{TARGET_TRIPLE}.exe")),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .ok_or(SidecarError::Missing { name, dir })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_directory_is_an_error_not_a_path_search() {
        let dir = std::env::temp_dir().join("blinkify-sidecar-empty-dir-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let error = Sidecar::in_dir(&dir).expect_err("nothing is there");
        assert!(matches!(
            error,
            SidecarError::Missing { name: "ffmpeg", .. }
        ));
    }

    #[test]
    fn both_installed_and_source_tree_names_resolve_to_absolute_paths() {
        let dir = std::env::temp_dir().join("blinkify-sidecar-names-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("ffmpeg.exe"), b"").expect("write");
        std::fs::write(dir.join(format!("ffprobe-{TARGET_TRIPLE}.exe")), b"").expect("write");

        let sidecar = Sidecar::in_dir(&dir).expect("both present");
        assert!(sidecar.ffmpeg().is_absolute());
        assert!(sidecar.ffmpeg().ends_with("ffmpeg.exe"));
        assert!(
            sidecar
                .ffprobe()
                .ends_with(format!("ffprobe-{TARGET_TRIPLE}.exe"))
        );
    }
}
