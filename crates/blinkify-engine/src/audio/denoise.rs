//! Noise reduction (#47): `RNNoise`, through FFmpeg's `arnndn`, with a model
//! that ships with Blinkify.
//!
//! - **The model is bundled, never fetched.** `arnndn` needs a model file at
//!   run time. It is installed with the application and found there
//!   ([`Models::in_dir`]); nothing looks for one on the system or the network.
//!   A missing or damaged file is an error that names it, not a silent
//!   export without noise reduction.
//! - **Strength is a blend, in the filter graph.** Full-strength `RNNoise` on
//!   clean sound gives the "underwater" artefact, so strength is `arnndn`'s
//!   own `mix` of the denoised signal with the original — the same graph the
//!   preview plays and the export writes, never two exports mixed afterwards.
//! - **Its delay is taken back.** `RNNoise` works in 10 ms frames at 48 kHz and
//!   emits each one a frame late. The chain pads the end by a frame and drops
//!   the first, so the sound stays on the sample it was on: at strength 0 the
//!   output is the input, bit for bit.
//! - **It is a speech denoiser.** The model was trained on speech against
//!   fans, air conditioning and computers. On music it removes what it takes
//!   for noise, and the inspector says so.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use thiserror::Error;

/// The bundled model: `somnolent-hogwash` from rnnoise-models, trained for
/// speech in a recording environment. See `THIRD_PARTY.md`.
pub const MODEL_FILE: &str = "sh.rnnn";

/// The model's SHA-256: a file that is present but not this one is damaged.
pub const MODEL_SHA256: &str = "70bb6685eb0c2a1d18e2918dca3fbfbd39317010b1802eb1b6ea73a92f3fdec0";

/// The only rate `arnndn` runs at.
pub const RATE: u32 = 48_000;

/// The samples `arnndn` delays its output by: one `RNNoise` frame.
pub const DELAY_SAMPLES: u32 = 480;

/// Where the bundled models are, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Models {
    rnnoise: Arc<PathBuf>,
}

/// Why the models could not be used.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ModelError {
    #[error(
        "the noise reduction model is missing from the installation: {0} was not found; reinstall Blinkify"
    )]
    Missing(PathBuf),
    #[error(
        "the noise reduction model at {0} is damaged: it is not the file Blinkify shipped; reinstall Blinkify"
    )]
    Damaged(PathBuf),
}

impl Models {
    /// The models installed in `dir`, each checked against the hash it
    /// shipped with.
    ///
    /// # Errors
    ///
    /// A model is absent, unreadable, or not the file that shipped.
    pub fn in_dir(dir: &Path) -> Result<Self, ModelError> {
        let path = dir.join(MODEL_FILE);
        let bytes = fs::read(&path).map_err(|_| ModelError::Missing(path.clone()))?;
        let digest = Sha256::digest(&bytes);
        let hex = digest.iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        if hex != MODEL_SHA256 {
            return Err(ModelError::Damaged(path));
        }
        Ok(Self {
            rnnoise: Arc::new(path),
        })
    }

    /// The `RNNoise` model file.
    #[must_use]
    pub fn rnnoise(&self) -> &Path {
        &self.rnnoise
    }
}

/// The filters that denoise at `strength` (0 to 1) and hand the sound back at
/// `sample_rate`, on the sample it came in on.
#[must_use]
pub fn filters(models: &Models, strength: f64, sample_rate: u32) -> String {
    format!(
        "aresample={RATE},apad=pad_len={DELAY_SAMPLES},arnndn=m={model}:mix={mix:.3},\
         atrim=start_sample={DELAY_SAMPLES},asetpts=PTS-STARTPTS,aresample={sample_rate}",
        model = quoted(models.rnnoise()),
        mix = strength.clamp(0.0, 1.0),
    )
}

/// `path` as an option value inside a filter graph. FFmpeg reads it twice:
/// the graph parser takes the filter's arguments, then the option parser
/// splits them. So the path is escaped for the second — its drive colon, a
/// quote or a backslash — and the result again for the first, which also
/// treats `,` `;` `[` `]` as structure. A path is never read as more of the
/// graph than itself (`CLAUDE.md` section 11).
fn quoted(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    escaped(&escaped(&text, r"\:'"), r"\',;[]")
}

fn escaped(text: &str, special: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if special.contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn models_at(path: &str) -> Models {
        Models {
            rnnoise: Arc::new(PathBuf::from(path)),
        }
    }

    #[test]
    fn a_windows_path_is_escaped_for_both_of_ffmpegs_parsers() {
        let chain = filters(
            &models_at(r"C:\Program Files\Blinkify\sh.rnnn"),
            0.5,
            44_100,
        );
        assert!(
            chain.contains(r"arnndn=m=C\\:/Program Files/Blinkify/sh.rnnn:mix=0.500"),
            "{chain}"
        );
        assert!(chain.starts_with("aresample=48000,apad=pad_len=480,"));
        assert!(chain.ends_with("atrim=start_sample=480,asetpts=PTS-STARTPTS,aresample=44100"));
    }

    #[test]
    fn a_quote_or_graph_punctuation_in_a_path_is_escaped_twice() {
        assert_eq!(
            quoted(Path::new("D:/it's, [odd]; dir/m")),
            r"D\\:/it\\\'s\, \[odd\]\; dir/m"
        );
    }

    #[test]
    fn strength_is_clamped_to_a_blend() {
        assert!(filters(&models_at("m"), 3.0, 48_000).contains(":mix=1.000,"));
        assert!(filters(&models_at("m"), -1.0, 48_000).contains(":mix=0.000,"));
    }

    #[test]
    fn a_missing_or_damaged_model_is_named() {
        let dir = std::env::temp_dir().join(format!("blinkify-models-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("dir");
        assert_eq!(
            Models::in_dir(&dir),
            Err(ModelError::Missing(dir.join(MODEL_FILE)))
        );
        fs::write(dir.join(MODEL_FILE), "rnnoise-nu model file version 1\n").expect("write");
        assert_eq!(
            Models::in_dir(&dir),
            Err(ModelError::Damaged(dir.join(MODEL_FILE)))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_bundled_model_is_the_one_that_shipped() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("models/rnnoise");
        let models = Models::in_dir(&dir).expect("the bundled model");
        assert!(models.rnnoise().ends_with(MODEL_FILE));
    }
}
