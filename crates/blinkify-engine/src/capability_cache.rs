//! The encoder capability profile, kept between launches.
//!
//! `ADR-0003` part 1: the profile is cached, keyed on the sidecar build and
//! the driver version, and re-probed when that key changes. Building it opens
//! every advertised hardware encoder at every profile, bit depth and level —
//! on a machine with an NVIDIA GPU, tens of seconds of trial encodes — and the
//! answer only changes when one of two things does:
//!
//! - **the sidecar**, which decides what is advertised and how each encoder is
//!   driven. Keyed on the SHA-256 of `ffmpeg.exe` itself, not its version
//!   string: a rebuild with a different configure line keeps the version.
//! - **a display driver**, which decides what a hardware encoder can do.
//!   Keyed on every adapter's driver version ([`display_driver`]).
//!
//! The key is stored inside the file and compared on every read. A profile
//! whose key does not match is never returned — not as a placeholder while the
//! new probe runs, not at all — because a profile from the old driver is the
//! "listed but fails at export" case ADR-0003 exists to prevent. The caller
//! reports "checking" until the new probe finishes.
//!
//! The file carries a format version. A file of another version, a truncated
//! file or one that is not JSON at all is discarded and the machine
//! re-probed: the cache is a shortcut, and the cost of distrusting it is one
//! probe, never an error.

use std::fs::File;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cache::Cache;
use crate::capability::{self, EncodeTrial, EncoderCapabilities};
use crate::display_driver::{self, DisplayDriver};
use crate::sidecar::Sidecar;

/// The format of the cache file. Bump it whenever [`EncoderCapabilities`] or
/// [`ProfileKey`] changes shape or meaning — including a new candidate
/// encoder in the probe, which a profile written before it would lack.
pub const FORMAT_VERSION: u32 = 1;

/// The directory under the cache root, and the one file in it.
const KIND: &str = "capabilities";
const FILE: &str = "encoders.json";

/// What the profile depends on. Two equal keys mean the same probe would find
/// the same encoders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileKey {
    /// SHA-256 of the bundled `ffmpeg.exe`, lower-case hex.
    pub sidecar_sha256: String,
    /// Every display adapter's driver, sorted.
    pub display_drivers: Vec<DisplayDriver>,
}

impl ProfileKey {
    /// The key of this machine as it is now: the sidecar's hash and the
    /// installed display drivers.
    ///
    /// # Errors
    ///
    /// `ffmpeg.exe` cannot be read.
    pub fn of_this_machine(sidecar: &Sidecar) -> io::Result<Self> {
        Ok(Self {
            sidecar_sha256: sha256_of(sidecar.ffmpeg())?,
            display_drivers: display_driver::installed(),
        })
    }
}

/// SHA-256 of a whole file, streamed.
fn sha256_of(path: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    io::copy(&mut File::open(path)?, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
            hex
        }))
}

/// The file as written.
#[derive(Serialize)]
struct Stored<'a> {
    version: u32,
    key: &'a ProfileKey,
    capabilities: &'a EncoderCapabilities,
}

/// The file as read back: the version first, on its own, so a file of another
/// version is recognised as such rather than misread as this one.
#[derive(Deserialize)]
struct Version {
    version: u32,
}

#[derive(Deserialize)]
struct Loaded {
    key: ProfileKey,
    capabilities: EncoderCapabilities,
}

/// The cached profile, if there is one for exactly `key`.
///
/// `None` for a missing, unreadable, truncated or corrupt file, a file of
/// another format version, and a profile probed under another key.
#[must_use]
pub fn cached(cache: &Cache, key: &ProfileKey) -> Option<EncoderCapabilities> {
    let bytes = cache.read(&cache.named(KIND, FILE))?;
    let Version { version } = serde_json::from_slice(&bytes).ok()?;
    if version != FORMAT_VERSION {
        return None;
    }
    let loaded: Loaded = serde_json::from_slice(&bytes).ok()?;
    (loaded.key == *key).then_some(loaded.capabilities)
}

/// Keep `capabilities` as the profile for `key`, replacing whatever was there.
///
/// # Errors
///
/// The cache directory cannot be written.
pub fn store(
    cache: &Cache,
    key: &ProfileKey,
    capabilities: &EncoderCapabilities,
) -> io::Result<()> {
    let bytes = serde_json::to_vec(&Stored {
        version: FORMAT_VERSION,
        key,
        capabilities,
    })
    .map_err(io::Error::other)?;
    cache.write(&cache.named(KIND, FILE), &bytes)
}

/// The profile for `key`: from the cache if it was probed under exactly this
/// key, otherwise probed now with `trial` and kept for next time.
///
/// Without a cache or a key the machine is simply probed — the answer is the
/// same, it just is not kept. A cache that cannot be written is not an error
/// either: the probe's answer is still correct for this launch.
#[must_use]
pub fn cached_or_probe(
    cache: Option<&Cache>,
    key: Option<&ProfileKey>,
    trial: &dyn EncodeTrial,
) -> EncoderCapabilities {
    if let (Some(cache), Some(key)) = (cache, key)
        && let Some(found) = cached(cache, key)
    {
        return found;
    }
    let found = capability::probe(trial);
    if let (Some(cache), Some(key)) = (cache, key) {
        let _ = store(cache, key, &found);
    }
    found
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::capability::{Trial, VideoCodec};

    /// A machine where every encoder works, counting every question asked of
    /// it — a count of zero is a launch that ran no trial at all.
    #[derive(Default)]
    struct CountingMachine {
        asked: AtomicUsize,
    }

    impl CountingMachine {
        fn asked(&self) -> usize {
            self.asked.load(Ordering::SeqCst)
        }
    }

    impl EncodeTrial for CountingMachine {
        fn advertised(&self) -> BTreeSet<String> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            ["hevc_nvenc", "libsvtav1"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        }
        fn succeeds(&self, _: &Trial<'_>) -> bool {
            self.asked.fetch_add(1, Ordering::SeqCst);
            true
        }
    }

    fn scratch(name: &str) -> Cache {
        let dir = std::env::temp_dir()
            .join("blinkify-capability-cache-tests")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        Cache::new(dir, 1 << 20)
    }

    fn file(cache: &Cache) -> PathBuf {
        cache.named(KIND, FILE)
    }

    fn key() -> ProfileKey {
        ProfileKey {
            sidecar_sha256: "a".repeat(64),
            display_drivers: vec![DisplayDriver {
                adapter: "NVIDIA GeForce RTX 3060 Laptop GPU".into(),
                version: "32.0.15.7688".into(),
            }],
        }
    }

    /// Probe once under `key()`, so the cache holds a profile.
    fn primed(name: &str) -> Cache {
        let cache = scratch(name);
        let machine = CountingMachine::default();
        let _ = cached_or_probe(Some(&cache), Some(&key()), &machine);
        assert!(machine.asked() > 0, "the first launch probes");
        assert!(file(&cache).is_file(), "and keeps what it found");
        cache
    }

    #[test]
    fn a_second_launch_with_the_same_key_runs_no_trial() {
        let cache = scratch("same-key");
        let first = CountingMachine::default();
        let probed = cached_or_probe(Some(&cache), Some(&key()), &first);
        assert!(first.asked() > 0);

        let second = CountingMachine::default();
        let reported = cached_or_probe(Some(&cache), Some(&key()), &second);
        assert_eq!(second.asked(), 0, "no encoder was opened");
        assert_eq!(reported, probed);
        assert_eq!(
            reported.encoders_for(VideoCodec::Hevc)[0].encoder,
            "hevc_nvenc"
        );
    }

    #[test]
    fn a_replaced_sidecar_re_probes() {
        let cache = primed("new-sidecar");
        let replaced = ProfileKey {
            sidecar_sha256: "b".repeat(64),
            ..key()
        };
        assert_eq!(
            cached(&cache, &replaced),
            None,
            "the old profile is not current"
        );
        let machine = CountingMachine::default();
        let _ = cached_or_probe(Some(&cache), Some(&replaced), &machine);
        assert!(machine.asked() > 0);
        assert!(
            cached(&cache, &replaced).is_some(),
            "the new profile is kept"
        );
        assert_eq!(cached(&cache, &key()), None, "and replaces the old one");
    }

    #[test]
    fn an_updated_driver_re_probes() {
        let cache = primed("new-driver");
        let mut updated = key();
        updated.display_drivers[0].version = "32.0.15.8000".into();
        assert_eq!(
            cached(&cache, &updated),
            None,
            "the old profile is not current"
        );
        let machine = CountingMachine::default();
        let _ = cached_or_probe(Some(&cache), Some(&updated), &machine);
        assert!(machine.asked() > 0);
    }

    #[test]
    fn a_new_adapter_re_probes() {
        let cache = primed("new-adapter");
        let mut added = key();
        added.display_drivers.push(DisplayDriver {
            adapter: "Intel(R) UHD Graphics".into(),
            version: "31.0.101.2141".into(),
        });
        added.display_drivers.sort();
        assert_eq!(cached(&cache, &added), None);
    }

    #[test]
    fn a_corrupt_or_truncated_file_re_probes_rather_than_failing() {
        let cache = primed("corrupt");
        let whole = fs::read(file(&cache)).expect("read");
        for damaged in [
            whole[..whole.len().div_euclid(2)].to_vec(),
            Vec::new(),
            b"\x00\xff not json".to_vec(),
            b"{\"version\":1}".to_vec(),
            b"[]".to_vec(),
        ] {
            fs::write(file(&cache), &damaged).expect("damage");
            assert_eq!(cached(&cache, &key()), None);
            let machine = CountingMachine::default();
            let found = cached_or_probe(Some(&cache), Some(&key()), &machine);
            assert!(machine.asked() > 0, "re-probed after {damaged:?}");
            assert_eq!(found.codecs.len(), 4);
            assert!(cached(&cache, &key()).is_some(), "and repaired the file");
        }
    }

    #[test]
    fn a_file_of_another_format_version_is_discarded_not_misread() {
        let cache = primed("version");
        let text = fs::read_to_string(file(&cache)).expect("read");
        let current = format!("\"version\":{FORMAT_VERSION}");
        assert!(text.contains(&current));
        // Every field this version reads is still there and still parses;
        // only the version says it means something else.
        let future = text.replace(&current, &format!("\"version\":{}", FORMAT_VERSION + 1));
        fs::write(file(&cache), future).expect("write");
        assert_eq!(cached(&cache, &key()), None);

        let machine = CountingMachine::default();
        let _ = cached_or_probe(Some(&cache), Some(&key()), &machine);
        assert!(machine.asked() > 0);
    }

    #[test]
    fn without_a_cache_or_a_key_the_machine_is_simply_probed() {
        let machine = CountingMachine::default();
        let found = cached_or_probe(None, Some(&key()), &machine);
        assert!(machine.asked() > 0);
        assert_eq!(found.encoders_for(VideoCodec::Av1)[0].encoder, "libsvtav1");

        let cache = scratch("no-key");
        let machine = CountingMachine::default();
        let _ = cached_or_probe(Some(&cache), None, &machine);
        assert!(machine.asked() > 0);
        assert!(!file(&cache).exists(), "nothing is kept without a key");
    }

    #[test]
    fn the_sidecar_hash_is_the_sha256_of_the_whole_file() {
        let dir = std::env::temp_dir()
            .join("blinkify-capability-cache-tests")
            .join("hash");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("ffmpeg.exe");
        fs::write(&path, b"abc").expect("write");
        // FIPS 180-2, appendix B.1.
        assert_eq!(
            sha256_of(&path).expect("hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
