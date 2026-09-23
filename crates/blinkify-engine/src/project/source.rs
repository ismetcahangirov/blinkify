//! Source references: which original file a clip plays, and whether it is
//! still the file the project was made with.
//!
//! A [`SourceRef`] is built from an [`ExportSource`], and nothing else — so
//! the only files the edit graph can name are originals. A proxy, a render or
//! a temporary file cannot be turned into one:
//!
//! ```compile_fail
//! # use blinkify_engine::project::SourceRef;
//! # use blinkify_engine::proxy::Proxy;
//! fn add(proxy: &Proxy) -> SourceRef {
//!     SourceRef::of(proxy) // takes an ExportSource, which a proxy is not
//! }
//! ```
//!
//! ```compile_fail
//! # use blinkify_engine::project::{Fingerprint, SourceRef};
//! // Nor can one be assembled around an arbitrary path: the fields are
//! // private.
//! let _ = SourceRef { path: "render.mp4".into(), fingerprint: todo!() };
//! ```
//!
//! The fingerprint is what tells a moved file from a changed one. The path
//! alone cannot: a file replaced at the same path is a different source, and
//! every cache keyed on it must know.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::cache::sampled_hash;
use crate::proxy::ExportSource;

/// What identifies one version of a file's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Fingerprint {
    #[ts(type = "number")]
    pub size: u64,
    /// Milliseconds since the Unix epoch. Recorded, and shown to the user
    /// when a source has changed, but not part of the identity: copying a
    /// file to another disk changes its modification time and nothing else.
    #[ts(type = "number | null")]
    pub modified: Option<u64>,
    /// SHA-256 over the size and the first and last mebibyte, truncated to
    /// 128 bits — the same sampling as the cache key (#24).
    pub content_hash: String,
}

impl Fingerprint {
    /// Fingerprint the file at `path`.
    ///
    /// # Errors
    ///
    /// The file cannot be opened or read.
    pub fn of(path: &Path) -> std::io::Result<Self> {
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        let size = metadata.len();
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|since| u64::try_from(since.as_millis()).ok());
        Ok(Self {
            size,
            modified,
            content_hash: sampled_hash(&mut file, size)?,
        })
    }

    /// Whether two fingerprints are the same content.
    #[must_use]
    pub fn same_content(&self, other: &Self) -> bool {
        self.size == other.size && self.content_hash == other.content_hash
    }
}

/// An original source file of the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceRef {
    #[ts(type = "string")]
    path: PathBuf,
    fingerprint: Fingerprint,
}

/// Whether a source is where the project left it, as it left it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "kebab-case")]
#[ts(export)]
pub enum SourceStatus {
    Present,
    /// Nothing at the path: moved, renamed, or on a drive that is not
    /// connected.
    Missing,
    /// Something is at the path, but not the content the project was made
    /// with.
    Changed {
        found: Fingerprint,
    },
}

impl SourceStatus {
    #[must_use]
    pub fn is_present(&self) -> bool {
        matches!(self, Self::Present)
    }
}

/// Why a file could not stand in for a source.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RelinkError {
    #[error("the file could not be read: {0}")]
    Unreadable(String),
    #[error("that file is not the one the project was made with")]
    DifferentContent,
}

impl SourceRef {
    /// Reference an original file, fingerprinting it now.
    ///
    /// # Errors
    ///
    /// The file cannot be opened or read.
    pub fn of(source: &ExportSource) -> std::io::Result<Self> {
        Ok(Self {
            path: source.path().to_path_buf(),
            fingerprint: Fingerprint::of(source.path())?,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Look at the file at the recorded path.
    #[must_use]
    pub fn check(&self) -> SourceStatus {
        if !self.path.is_file() {
            return SourceStatus::Missing;
        }
        match Fingerprint::of(&self.path) {
            Ok(found) if found.same_content(&self.fingerprint) => SourceStatus::Present,
            Ok(found) => SourceStatus::Changed { found },
            // There, but unreadable — locked, or permissions: to the user
            // this is the same as gone.
            Err(_) => SourceStatus::Missing,
        }
    }

    /// Point the reference at `candidate`, if it has the same content — the
    /// file moved, or the drive has a new letter.
    ///
    /// # Errors
    ///
    /// The candidate cannot be read, or is a different file.
    pub fn relink(&self, candidate: &ExportSource) -> Result<Self, RelinkError> {
        let found = Fingerprint::of(candidate.path())
            .map_err(|error| RelinkError::Unreadable(error.to_string()))?;
        if !found.same_content(&self.fingerprint) {
            return Err(RelinkError::DifferentContent);
        }
        Ok(Self {
            path: candidate.path().to_path_buf(),
            fingerprint: found,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(path: &str, fingerprint: Fingerprint) -> Self {
        Self {
            path: PathBuf::from(path),
            fingerprint,
        }
    }
}
