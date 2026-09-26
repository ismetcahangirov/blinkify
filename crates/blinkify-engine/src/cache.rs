//! The on-disk cache for derived artefacts: keyframe indices, waveform peaks,
//! filmstrips.
//!
//! `CLAUDE.md` section 12: caches are on disk and **keyed by content, not by
//! path**. Moving a file must not throw away its waveform; editing it must.
//! So an entry's key is a hash of the file's first and last mebibyte plus its
//! size, together with its modification time — which survives a move or a
//! rename on the same volume, and changes on any edit.
//!
//! Hashing the whole file would be exact and would read a two-hour recording
//! end to end just to look something up. The sampled hash plus size plus mtime
//! is what the issue asks for, and a false match needs a file edited without
//! its size, its mtime or either end changing.
//!
//! Section 20 rule 1 does not apply here: nothing in this cache reaches an
//! output file. These are derived artefacts, and the directory has a size
//! budget with least-recently-used eviction — an unbounded cache is a support
//! ticket.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// Bytes hashed from each end of the file.
const SAMPLE: u64 = 1024 * 1024;

/// The identity of one version of one file's content.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentKey(String);

impl ContentKey {
    /// Hash `path`'s first and last mebibyte, its size and its modification
    /// time.
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
            .map_or(0, |since| since.as_nanos());

        let hex = sampled_hash(&mut file, size)?;
        Ok(Self(format!("{hex}-{size}-{modified}")))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A hash of `file`'s size and its first and last mebibyte: the part of a
/// content key that does not depend on the file system. The project file
/// (#32) stores it on its own, so a touched but unchanged source still
/// matches.
pub(crate) fn sampled_hash(file: &mut File, size: u64) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());
    let mut buffer = Vec::new();
    Read::by_ref(file).take(SAMPLE).read_to_end(&mut buffer)?;
    hasher.update(&buffer);
    if size > SAMPLE {
        buffer.clear();
        file.seek(SeekFrom::Start(size.saturating_sub(SAMPLE).max(SAMPLE)))?;
        Read::by_ref(file).take(SAMPLE).read_to_end(&mut buffer)?;
        hasher.update(&buffer);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().take(16).fold(String::new(), |mut hex, byte| {
        let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
        hex
    }))
}

/// A cache directory with a size budget.
#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
    budget_bytes: u64,
}

impl Cache {
    /// A cache rooted at `root` — the OS cache directory in the application,
    /// a temporary directory in tests — holding at most `budget_bytes`.
    #[must_use]
    pub fn new(root: PathBuf, budget_bytes: u64) -> Self {
        Self { root, budget_bytes }
    }

    /// Whether `path` is a file inside this cache — what may be served from
    /// it. Compared after resolving `..` and links, so a path that climbs out
    /// of the cache is not in it.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        match (path.canonicalize(), self.root.canonicalize()) {
            (Ok(path), Ok(root)) => path.starts_with(root) && path.is_file(),
            _ => false,
        }
    }

    /// Where the entry for `key` in `kind` lives. `kind` separates artefacts
    /// (`keyframes`, `peaks`, `filmstrip`) that share a key.
    #[must_use]
    pub fn path(&self, kind: &str, key: &ContentKey, suffix: &str) -> PathBuf {
        self.root
            .join(kind)
            .join(format!("{}{suffix}", key.as_str()))
    }

    /// Where the one entry `name` in `kind` lives — for an artefact there is
    /// only ever one of, which carries its own key inside it, such as the
    /// encoder capability profile (#83).
    #[must_use]
    pub fn named(&self, kind: &str, name: &str) -> PathBuf {
        self.root.join(kind).join(name)
    }

    /// Read an entry, marking it recently used.
    #[must_use]
    pub fn read(&self, path: &Path) -> Option<Vec<u8>> {
        let bytes = fs::read(path).ok()?;
        // Recency for eviction is the file's modification time; touching it on
        // read makes the ordering least-recently *used*, not least-recently
        // written.
        if let Ok(file) = File::options().write(true).open(path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(bytes)
    }

    /// Write an entry atomically, then evict down to the budget.
    ///
    /// Written to a temporary name and renamed, so a crash mid-write leaves
    /// either the old entry or none — never a truncated one that a later read
    /// would misparse.
    ///
    /// # Errors
    ///
    /// The cache directory cannot be written.
    pub fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let partial = path.with_extension("partial");
        {
            let mut file = File::create(&partial)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(&partial, path)?;
        self.evict(Some(path));
        Ok(())
    }

    /// Delete least-recently-used entries until the cache fits its budget.
    /// `keep` is never evicted — the entry just written is the one about to
    /// be used.
    pub fn evict(&self, keep: Option<&Path>) {
        let mut entries: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
        collect(&self.root, &mut entries);
        let mut total: u64 = entries.iter().map(|(_, size, _)| size).sum();
        if total <= self.budget_bytes {
            return;
        }
        entries.sort_by_key(|(_, _, used)| *used);
        for (path, size, _) in entries {
            if total <= self.budget_bytes {
                break;
            }
            if Some(path.as_path()) == keep {
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }

    /// Total bytes on disk, for tests and for a settings page.
    #[must_use]
    pub fn size_on_disk(&self) -> u64 {
        let mut entries = Vec::new();
        collect(&self.root, &mut entries);
        entries.iter().map(|(_, size, _)| size).sum()
    }
}

fn collect(dir: &Path, into: &mut Vec<(PathBuf, u64, SystemTime)>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect(&entry.path(), into);
        } else {
            into.push((
                entry.path(),
                metadata.len(),
                metadata.modified().unwrap_or(UNIX_EPOCH),
            ));
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("blinkify-cache-tests").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn a_moved_file_keeps_its_key_and_an_edited_one_does_not() {
        let dir = scratch("key");
        let original = dir.join("a.bin");
        fs::write(&original, vec![7_u8; 3 * 1024 * 1024]).expect("write");
        let before = ContentKey::of(&original).expect("key");

        let moved = dir.join("renamed.bin");
        fs::rename(&original, &moved).expect("rename");
        assert_eq!(ContentKey::of(&moved).expect("key"), before);

        let mut bytes = fs::read(&moved).expect("read");
        bytes[10] = 8;
        fs::write(&moved, &bytes).expect("edit");
        assert_ne!(ContentKey::of(&moved).expect("key"), before);
    }

    #[test]
    fn eviction_keeps_the_cache_within_its_budget_oldest_first() {
        let dir = scratch("evict");
        let cache = Cache::new(dir.clone(), 2_500);
        let key = |n: u8| ContentKey(format!("k{n}"));
        for n in 0..5 {
            let path = cache.path("peaks", &key(n), ".bin");
            cache.write(&path, &[n; 1_000]).expect("write");
            // Distinct modification times, so the order is unambiguous.
            let file = File::options().write(true).open(&path).expect("open");
            file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(u64::from(n) * 60))
                .expect("mtime");
        }
        cache.evict(None);
        assert!(cache.size_on_disk() <= 2_500);
        assert!(cache.path("peaks", &key(4), ".bin").exists(), "newest kept");
        assert!(
            !cache.path("peaks", &key(0), ".bin").exists(),
            "oldest evicted"
        );
    }

    #[test]
    fn a_read_counts_as_a_use() {
        let dir = scratch("lru");
        let cache = Cache::new(dir, 2_500);
        let old = cache.path("k", &ContentKey("old".into()), "");
        let new = cache.path("k", &ContentKey("new".into()), "");
        cache.write(&old, &[0; 1_000]).expect("write");
        File::options()
            .write(true)
            .open(&old)
            .expect("open")
            .set_modified(UNIX_EPOCH)
            .expect("mtime");
        cache.write(&new, &[0; 1_000]).expect("write");
        assert!(cache.read(&old).is_some());
        let third = cache.path("k", &ContentKey("third".into()), "");
        cache.write(&third, &[0; 1_000]).expect("write");
        assert!(old.exists(), "read recently, so kept");
    }
}
