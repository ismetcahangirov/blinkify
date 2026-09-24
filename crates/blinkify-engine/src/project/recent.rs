//! Recently opened projects (#54): a preference of the user's, kept in the
//! application's settings, never in a project file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How many projects the list keeps.
pub const RECENT_LIMIT: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecentProject {
    #[ts(type = "string")]
    pub path: PathBuf,
    pub name: String,
    /// When it was last opened or saved, ms since the Unix epoch.
    #[ts(type = "number")]
    pub opened_at: u64,
}

/// The list, most recent first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentProjects {
    pub entries: Vec<RecentProject>,
}

impl RecentProjects {
    /// Read the list from `file`; an unreadable or damaged list is an empty
    /// one — it is a convenience, and losing it costs a click.
    #[must_use]
    pub fn load(file: &Path) -> Self {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Write the list to `file`.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(file, text)
    }

    /// Put `path` first, once, and keep at most [`RECENT_LIMIT`].
    pub fn touch(&mut self, path: &Path, name: &str, now: u64) {
        self.entries.retain(|entry| !same_path(&entry.path, path));
        self.entries.insert(
            0,
            RecentProject {
                path: path.to_path_buf(),
                name: name.to_owned(),
                opened_at: now,
            },
        );
        self.entries.truncate(RECENT_LIMIT);
    }

    /// The paths, most recent first.
    #[must_use]
    pub fn paths(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect()
    }
}

/// Windows paths are not case-sensitive: `C:\Work\Trip.blinkify` and
/// `c:\work\trip.blinkify` are one project.
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_is_first_once_and_the_list_is_bounded() {
        let mut recent = RecentProjects::default();
        for n in 0..15_u64 {
            recent.touch(&PathBuf::from(format!("C:\\p{n}.blinkify")), "p", n);
        }
        assert_eq!(recent.entries.len(), RECENT_LIMIT);
        assert_eq!(recent.entries[0].opened_at, 14);
        recent.touch(Path::new("c:\\P10.BLINKIFY"), "again", 20);
        assert_eq!(recent.entries.len(), RECENT_LIMIT);
        assert_eq!(recent.entries[0].name, "again");
        assert_eq!(
            recent
                .entries
                .iter()
                .filter(|e| e
                    .path
                    .to_string_lossy()
                    .to_lowercase()
                    .ends_with("p10.blinkify"))
                .count(),
            1
        );
    }

    #[test]
    fn it_survives_a_round_trip_and_a_damaged_file_is_empty() {
        let dir = std::env::temp_dir().join("blinkify-recent");
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("recent.json");
        let mut recent = RecentProjects::default();
        recent.touch(Path::new("C:\\Trip.blinkify"), "Trip", 5);
        recent.save(&file).expect("save");
        assert_eq!(RecentProjects::load(&file), recent);
        std::fs::write(&file, "not json").expect("write");
        assert_eq!(RecentProjects::load(&file), RecentProjects::default());
    }
}
