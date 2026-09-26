//! What the export writes: the evaluated edit graph over original files.
//!
//! The export planner (#39) starts here, from the same [`Timeline`] the
//! preview plays (#30) — so the two cannot mean different things by the same
//! graph. What differs is deliberate and in the types: the preview's sources
//! are [`SourceMedia`](crate::playback::SourceMedia), which may carry a
//! proxy, and the export's are [`ExportSource`]s, which cannot.
//!
//! [`plan`] decides, segment by segment, what the export copies and what it
//! re-encodes (#39).

pub mod audio;
pub mod execute;
pub mod facts;
pub mod loudness;
pub mod nut;
pub mod plan;
pub mod profile;
pub mod queue;
pub mod render;
pub mod seam;
pub mod sps;
pub mod verify;

use std::collections::BTreeMap;

use crate::project::evaluate::{EvaluateError, OperationsAt, Timeline, evaluate};
use crate::project::{Project, SourceId};
use crate::proxy::{ExportSource, MediaAsset};

/// The content of an export: every clip resolved, every source an original.
#[derive(Debug, Clone)]
pub struct ExportContent {
    pub timeline: Timeline,
    pub sources: BTreeMap<SourceId, ExportSource>,
}

impl ExportContent {
    /// Evaluate `project` for export.
    ///
    /// # Errors
    ///
    /// The graph cannot be evaluated; see [`evaluate`].
    pub fn of(project: &Project) -> Result<Self, EvaluateError> {
        let timeline = evaluate(project)?;
        let sources = project
            .sources
            .iter()
            .map(|(&id, source)| {
                (
                    id,
                    MediaAsset::new(source.path().to_path_buf()).export_source(),
                )
            })
            .collect();
        Ok(Self { timeline, sources })
    }

    /// What the export applies at sequence frame `position`.
    #[must_use]
    pub fn at(&self, position: i64) -> OperationsAt {
        self.timeline.at(position)
    }
}
