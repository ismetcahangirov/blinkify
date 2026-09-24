//! The open project's life: new, open, save, save as, autosave and recovery
//! after an unclean shutdown (#54).
//!
//! Losing an edit to a crash is the most damaging thing an editor can do to
//! a user, so the rules are:
//!
//! - **Autosave never touches the user's project file.** It writes a sidecar
//!   recovery file — `Trip.blinkify.recovery` beside `Trip.blinkify`, or a
//!   file in the application's recovery directory for a project never saved —
//!   written to a temporary name and renamed into place. Only an explicit
//!   save writes the project file, and a save removes the recovery file.
//! - **Dirty is exact.** Serialisation is deterministic (#32), so a project is
//!   dirty exactly when its text differs from the text last saved or opened —
//!   undoing back to the saved state makes it clean again — and an autosave
//!   of text already autosaved is skipped.
//! - **A recovery is offered with both times.** [`scan`] finds recovery files
//!   newer than their project, and the offer carries when each was written,
//!   so the user is not asked to guess.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use super::edit::Document;
use super::{EXTENSION, Project, ProjectError};

/// The recovery file's suffix, after the project file's name.
pub const RECOVERY_SUFFIX: &str = "recovery";

/// Why a save did not happen. The project is unchanged, and still dirty.
#[derive(Debug, Error, PartialEq)]
pub enum SaveError {
    /// The project has never been saved: it needs a path first.
    #[error("the project has not been saved yet: choose where to save it")]
    Untitled,
    #[error(transparent)]
    Project(#[from] ProjectError),
}

/// The open project: its document, where it lives, and what was last saved.
#[derive(Debug)]
pub struct Session {
    path: Option<PathBuf>,
    document: Document,
    /// The project's text as last opened or saved: what "dirty" is against.
    saved: String,
    /// The text last autosaved, so an unchanged project is not written again.
    autosaved: Option<String>,
    /// Where autosaves go.
    recovery: PathBuf,
}

/// Milliseconds since the Unix epoch of a file's modification.
fn modified(path: &Path) -> Option<u64> {
    let time = std::fs::metadata(path).ok()?.modified().ok()?;
    u64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos())
}

/// The recovery file of the project file at `path`: beside it.
#[must_use]
pub fn recovery_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(RECOVERY_SUFFIX);
    path.with_file_name(name)
}

/// Write `text` to `path` through a temporary sibling renamed over it, so a
/// crash mid-write leaves the previous file whole.
fn write_atomically(path: &Path, text: &str) -> Result<(), ProjectError> {
    let mut partial = path.as_os_str().to_os_string();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    std::fs::write(&partial, text).map_err(|error| ProjectError::Io(error.to_string()))?;
    std::fs::rename(&partial, path).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        ProjectError::Io(error.to_string())
    })
}

impl Session {
    /// A new, untitled project; its autosaves go to `recovery_dir`.
    ///
    /// # Errors
    ///
    /// The project is inconsistent.
    pub fn untitled(project: Project, recovery_dir: &Path) -> Result<Self, ProjectError> {
        let saved = project.to_json()?;
        Ok(Self {
            path: None,
            document: Document::new(project)?,
            saved,
            autosaved: None,
            recovery: recovery_dir.join(format!(
                "untitled-{}.{EXTENSION}.{RECOVERY_SUFFIX}",
                now_nanos()
            )),
        })
    }

    /// Open the project file at `path`.
    ///
    /// # Errors
    ///
    /// As [`Project::load`]: unreadable, damaged, or from a newer build.
    pub fn open(path: &Path) -> Result<Self, ProjectError> {
        let saved =
            std::fs::read_to_string(path).map_err(|error| ProjectError::Io(error.to_string()))?;
        let project = Project::from_json(&saved)?;
        // What this build would write: a file of an older schema is dirty
        // until it is saved in this one, which is honest — saving changes it.
        Ok(Self {
            path: Some(path.to_path_buf()),
            document: Document::new(project)?,
            saved,
            autosaved: None,
            recovery: recovery_path_for(path),
        })
    }

    /// Reopen from a recovery file: the recovered edits, for the project the
    /// recovery belongs to, dirty until saved. The recovery file stays until
    /// then.
    ///
    /// # Errors
    ///
    /// The recovery file cannot be read or parsed.
    pub fn restore(offer: &RecoveryOffer) -> Result<Self, ProjectError> {
        let text = std::fs::read_to_string(&offer.recovery)
            .map_err(|error| ProjectError::Io(error.to_string()))?;
        let project = Project::from_json(&text)?;
        let saved = offer
            .project
            .as_deref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        Ok(Self {
            path: offer.project.clone(),
            document: Document::new(project)?,
            saved,
            autosaved: Some(text),
            recovery: offer.recovery.clone(),
        })
    }

    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// The document, to edit.
    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    /// Where the project file is; `None` until it is first saved.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether the project differs from what was last opened or saved.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.document
            .project()
            .to_json()
            .map_or(true, |text| text != self.saved)
    }

    /// Save to the project file.
    ///
    /// # Errors
    ///
    /// The project was never saved, or the file cannot be written — a
    /// read-only folder, a full disk. The project stays dirty.
    pub fn save(&mut self) -> Result<(), SaveError> {
        let path = self.path.clone().ok_or(SaveError::Untitled)?;
        self.save_to(&path)
    }

    /// Save to `path`, which becomes the project file.
    ///
    /// # Errors
    ///
    /// The file cannot be written; the project and its path are unchanged.
    pub fn save_as(&mut self, path: &Path) -> Result<(), SaveError> {
        self.save_to(path)?;
        if self.path.as_deref() != Some(path) {
            let _ = std::fs::remove_file(&self.recovery);
            self.path = Some(path.to_path_buf());
            self.recovery = recovery_path_for(path);
        }
        Ok(())
    }

    fn save_to(&mut self, path: &Path) -> Result<(), SaveError> {
        let text = self.document.project().to_json()?;
        write_atomically(path, &text)?;
        self.saved = text;
        // Saved: there is nothing left to recover.
        let _ = std::fs::remove_file(&self.recovery);
        self.autosaved = None;
        Ok(())
    }

    /// Write the recovery file if the project has changed since the last
    /// save and since the last autosave. Returns whether it wrote. A clean
    /// project leaves no recovery file behind.
    ///
    /// # Errors
    ///
    /// The recovery file cannot be written.
    pub fn autosave(&mut self) -> Result<bool, ProjectError> {
        let text = self.document.project().to_json()?;
        if text == self.saved {
            let _ = std::fs::remove_file(&self.recovery);
            self.autosaved = None;
            return Ok(false);
        }
        if self.autosaved.as_deref() == Some(text.as_str()) {
            return Ok(false);
        }
        if let Some(parent) = self.recovery.parent() {
            std::fs::create_dir_all(parent).map_err(|error| ProjectError::Io(error.to_string()))?;
        }
        write_atomically(&self.recovery, &text)?;
        self.autosaved = Some(text);
        Ok(true)
    }

    /// Close without saving, and forget the unsaved changes: the recovery
    /// file goes too. What "Don't save" means.
    pub fn discard(self) {
        let _ = std::fs::remove_file(&self.recovery);
    }
}

/// Unsaved work found at launch: a recovery file newer than its project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecoveryOffer {
    #[ts(type = "string")]
    pub recovery: PathBuf,
    /// The project file it recovers; `None` for one never saved.
    #[ts(type = "string | null")]
    pub project: Option<PathBuf>,
    /// The project's name, as the recovery has it.
    pub name: String,
    /// When the recovery was written, ms since the Unix epoch.
    #[ts(type = "number")]
    pub recovered_at: u64,
    /// When the project file was last saved; `None` if it is not there.
    #[ts(type = "number | null")]
    pub saved_at: Option<u64>,
}

fn offer(recovery: &Path, project: Option<&Path>) -> Option<RecoveryOffer> {
    let recovered_at = modified(recovery)?;
    let saved_at = project.and_then(modified);
    if saved_at.is_some_and(|saved| saved >= recovered_at) {
        return None;
    }
    let name = std::fs::read_to_string(recovery)
        .ok()
        .and_then(|text| Project::from_json(&text).ok())
        .map(|project| project.name)?;
    Some(RecoveryOffer {
        recovery: recovery.to_path_buf(),
        project: project.map(Path::to_path_buf),
        name,
        recovered_at,
        saved_at,
    })
}

/// Unsaved work to offer at launch: the recovery file beside each of
/// `projects` that is newer than it (or whose project is gone), and every
/// recovery of a never-saved project in `untitled_dir`.
#[must_use]
pub fn scan(projects: &[PathBuf], untitled_dir: &Path) -> Vec<RecoveryOffer> {
    let mut offers: Vec<RecoveryOffer> = projects
        .iter()
        .filter_map(|project| offer(&recovery_path_for(project), Some(project)))
        .collect();
    if let Ok(entries) = std::fs::read_dir(untitled_dir) {
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(&format!(".{EXTENSION}.{RECOVERY_SUFFIX}")))
            })
            .collect();
        found.sort();
        offers.extend(found.iter().filter_map(|path| offer(path, None)));
    }
    offers
}

/// Forget an offered recovery.
///
/// # Errors
///
/// The recovery file could not be removed.
pub fn discard_recovery(offer: &RecoveryOffer) -> Result<(), ProjectError> {
    std::fs::remove_file(&offer.recovery).map_err(|error| ProjectError::Io(error.to_string()))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::project::SequenceSettings;
    use crate::project::edit::{Edit, EditContext};
    use sha2::{Digest, Sha256};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("blinkify-session").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn rename(session: &mut Session, name: &str) {
        session
            .document_mut()
            .apply(
                &Edit::Rename {
                    name: name.to_owned(),
                },
                &EditContext::default(),
            )
            .expect("rename");
    }

    fn hash(path: &Path) -> Vec<u8> {
        Sha256::digest(std::fs::read(path).expect("read")).to_vec()
    }

    #[test]
    fn dirty_follows_the_edits_undo_redo_and_save() {
        let dir = scratch("dirty");
        let mut session = Session::untitled(Project::matching_first_clip("A"), &dir).expect("new");
        assert!(!session.is_dirty());
        rename(&mut session, "B");
        assert!(session.is_dirty());
        session.document_mut().undo();
        assert!(!session.is_dirty(), "back to what it was");
        session.document_mut().redo();
        assert!(session.is_dirty());
        assert_eq!(session.save(), Err(SaveError::Untitled));
        let path = dir.join("B.blinkify");
        session.save_as(&path).expect("save as");
        assert!(!session.is_dirty());
        assert_eq!(session.path(), Some(path.as_path()));
        session.document_mut().undo();
        assert!(session.is_dirty(), "the saved state is the one on disk");
    }

    #[test]
    fn autosave_writes_beside_the_project_and_never_touches_it() {
        let dir = scratch("autosave");
        let path = dir.join("Trip.blinkify");
        Project::new("Trip", SequenceSettings::default())
            .save(&path)
            .expect("save");
        let before = hash(&path);
        let mut session = Session::open(&path).expect("open");
        assert!(!session.autosave().expect("clean"), "nothing to recover");
        rename(&mut session, "Trip 2");
        assert!(session.autosave().expect("autosave"));
        assert!(!session.autosave().expect("again"), "unchanged: skipped");
        assert_eq!(hash(&path), before, "the project file is untouched");
        let recovery = recovery_path_for(&path);
        assert!(recovery.is_file());
        let recovered = Project::load(&recovery).expect("recovery is a project");
        assert_eq!(recovered.name, "Trip 2");
        // Saving makes the recovery unnecessary.
        session.save().expect("save");
        assert!(!recovery.exists());
        assert_ne!(hash(&path), before);
    }

    #[test]
    fn an_unclean_shutdown_is_offered_back_with_both_times() {
        let dir = scratch("recover");
        let path = dir.join("Trip.blinkify");
        Project::new("Trip", SequenceSettings::default())
            .save(&path)
            .expect("save");
        let untitled = dir.join("untitled");
        {
            let mut session = Session::open(&path).expect("open");
            rename(&mut session, "Edited");
            std::thread::sleep(std::time::Duration::from_millis(20));
            session.autosave().expect("autosave");
            let mut fresh =
                Session::untitled(Project::matching_first_clip("New"), &untitled).expect("new");
            rename(&mut fresh, "Never saved");
            fresh.autosave().expect("autosave");
            // The process dies here: nothing closes, nothing saves.
            std::mem::forget(session);
            std::mem::forget(fresh);
        }
        let offers = scan(std::slice::from_ref(&path), &untitled);
        assert_eq!(offers.len(), 2, "{offers:?}");
        let project = &offers[0];
        assert_eq!(project.name, "Edited");
        assert_eq!(project.project.as_deref(), Some(path.as_path()));
        assert!(
            project
                .saved_at
                .is_some_and(|saved| saved < project.recovered_at)
        );
        assert_eq!(offers[1].name, "Never saved");
        assert_eq!(offers[1].project, None);

        // Restore: the edits are back, dirty, and the recovery file stays
        // until they are saved.
        let restored = Session::restore(project).expect("restore");
        assert_eq!(restored.document().project().name, "Edited");
        assert!(restored.is_dirty());
        assert!(project.recovery.exists());
        // Discard: the offer goes, and is not made again.
        discard_recovery(&offers[1]).expect("discard");
        assert_eq!(scan(std::slice::from_ref(&path), &untitled).len(), 1);
    }

    #[test]
    fn an_older_recovery_than_the_saved_project_is_not_offered() {
        let dir = scratch("stale");
        let path = dir.join("Trip.blinkify");
        let recovery = recovery_path_for(&path);
        Project::new("Old edit", SequenceSettings::default())
            .save(&recovery)
            .expect("recovery");
        std::thread::sleep(std::time::Duration::from_millis(20));
        Project::new("Trip", SequenceSettings::default())
            .save(&path)
            .expect("save");
        assert!(scan(&[path], &dir.join("none")).is_empty());
    }

    #[test]
    fn a_save_that_cannot_write_leaves_the_project_dirty_and_the_file_whole() {
        let dir = scratch("readonly");
        let path = dir.join("Trip.blinkify");
        Project::new("Trip", SequenceSettings::default())
            .save(&path)
            .expect("save");
        let before = hash(&path);
        let mut session = Session::open(&path).expect("open");
        rename(&mut session, "Changed");
        // A folder where the file should be: the rename over it fails, as a
        // read-only or full disk would fail the write.
        let blocked = dir.join("blocked.blinkify");
        std::fs::create_dir_all(blocked.join("inside")).expect("dir");
        assert!(matches!(
            session.save_as(&blocked),
            Err(SaveError::Project(ProjectError::Io(_)))
        ));
        assert!(session.is_dirty());
        assert_eq!(session.path(), Some(path.as_path()));
        assert_eq!(hash(&path), before);
        // The project file deleted while open: saving puts it back.
        std::fs::remove_file(&path).expect("delete");
        session.save().expect("save again");
        assert_eq!(Project::load(&path).expect("load").name, "Changed");
    }

    #[test]
    fn a_project_from_a_newer_build_is_refused_whole() {
        let dir = scratch("future");
        let path = dir.join("Future.blinkify");
        std::fs::write(&path, "{\"schemaVersion\": 999, \"name\": \"x\"}").expect("write");
        assert!(matches!(
            Session::open(&path),
            Err(ProjectError::FutureVersion { found: 999, .. })
        ));
    }

    #[test]
    fn discarding_forgets_the_unsaved_changes() {
        let dir = scratch("discard");
        let path = dir.join("Trip.blinkify");
        Project::new("Trip", SequenceSettings::default())
            .save(&path)
            .expect("save");
        let mut session = Session::open(&path).expect("open");
        rename(&mut session, "Changed");
        session.autosave().expect("autosave");
        session.discard();
        assert!(!recovery_path_for(&path).exists());
        assert_eq!(Project::load(&path).expect("load").name, "Trip");
    }
}
