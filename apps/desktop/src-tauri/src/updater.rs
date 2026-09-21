//! The update check.
//!
//! `CLAUDE.md` section 20 rule 8 permits exactly one outbound request in the
//! whole application, and this is it. Rule 3 governs what happens next: nothing
//! is downloaded, installed or restarted without the user saying so.
//!
//! # The shape, and why it is this shape
//!
//! The check runs once at launch and then **stops**. It writes what it found
//! into application state and does nothing else — no download, no prompt of its
//! own, no restart. The renderer asks for the result with
//! [`pending_update`] and calls [`install_update`] only after a person has
//! clicked something.
//!
//! Splitting it that way is deliberate. An updater that decides for itself when
//! to apply an update is an updater that will one day replace a working build
//! with a broken one while somebody is mid-edit on a project they have not
//! saved. The two commands make "found an update" and "apply the update" two
//! separate events with a human in between, and no code path joins them.
//!
//! # Failure is not an error the user needs to see
//!
//! A failed check means no network, a GitHub outage, or no release yet. None of
//! those is the user's problem and none of them should produce a dialog on
//! launch, so a failure leaves the pending offer empty and is logged. The
//! application works exactly as well without an update as with one.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use ts_rs::TS;

/// An available update, as the renderer sees it.
///
/// Deliberately not the plugin's own `Update`: that type carries the download
/// URL and the signature, which the renderer has no business holding. It
/// carries what a person needs in order to answer "do I want this?".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateOffer {
    /// The version on offer.
    pub version: String,
    /// The version currently running, so the renderer can state both rather
    /// than asking the user to remember one of them.
    pub current_version: String,
    /// Release notes, when the release has them.
    pub notes: Option<String>,
    /// Publication date, as the manifest reports it.
    pub published_at: Option<String>,
}

/// What the launch check found, if anything.
///
/// The plugin's `Update` is kept rather than rebuilt later, because it holds the
/// verified download URL and signature from the manifest that was checked. Going
/// back to the network at install time would mean verifying a *second*, possibly
/// different manifest — and the user's consent was given for the first one.
#[derive(Default)]
pub struct PendingUpdate(pub Mutex<Option<Update>>);

impl std::fmt::Debug for PendingUpdate {
    /// Reports only whether an update is pending.
    ///
    /// The plugin's `Update` holds the download URL and the manifest signature.
    /// A derived `Debug` would put both into the first log line somebody adds
    /// while chasing an unrelated bug, and a signature in a log is a signature
    /// in a bug report.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self.0.lock() {
            Ok(guard) => {
                if guard.is_some() {
                    "pending"
                } else {
                    "none"
                }
            }
            Err(_) => "poisoned",
        };
        f.debug_tuple("PendingUpdate").field(&state).finish()
    }
}

/// The offer the launch check found, or `None`.
///
/// Cheap and synchronous: it reads state, it does not go to the network. The
/// renderer may call it whenever it mounts.
///
/// # Errors
///
/// Returns an error only if the state lock is poisoned, which means another
/// thread panicked while holding it.
#[tauri::command]
// Tauri injects managed state by value; a reference is not part of the command
// signature it accepts. Same trade as `install_update` below.
#[allow(clippy::needless_pass_by_value)]
pub fn pending_update(state: State<'_, PendingUpdate>) -> Result<Option<UpdateOffer>, String> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "the pending-update lock is poisoned".to_owned())?;

    Ok(guard.as_ref().map(|update| UpdateOffer {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
        published_at: update.date.map(|date| date.to_string()),
    }))
}

/// Download the pending update, install it, and restart into it.
///
/// Only ever reached because a person asked for it. The signature on the
/// manifest was verified during the check; the downloaded bytes are verified
/// again by the plugin before anything is written.
///
/// Does not return on success: the application restarts.
///
/// # Errors
///
/// Returns an error if no update is pending, if the state lock is poisoned, or
/// if the download or signature verification fails.
#[tauri::command]
// Tauri injects the handle and the managed state by value; a reference is not
// part of the command signature it accepts. `restart` only borrows, hence the
// lint, but neither parameter can become a reference without the command
// ceasing to be one.
#[allow(clippy::needless_pass_by_value)]
pub async fn install_update(app: AppHandle, state: State<'_, PendingUpdate>) -> Result<(), String> {
    // Taken out of the state rather than borrowed, so a second click cannot
    // start a second install over the first one.
    let update = {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "the pending-update lock is poisoned".to_owned())?;
        guard.take()
    };

    let Some(update) = update else {
        return Err("no update is pending".to_owned());
    };

    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|error| format!("the update could not be installed: {error}"))?;

    app.restart();
}

/// Check for an update once, at launch, and remember what was found.
///
/// Spawned rather than awaited: the window must appear whether or not GitHub
/// answers, and a slow network must not become a slow launch.
pub fn check_on_launch(app: &AppHandle) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        let updater = match app.updater() {
            Ok(updater) => updater,
            Err(error) => {
                // Configuration, not circumstance: a missing endpoint or public
                // key. Worth saying loudly, because it means this build can
                // never be updated.
                eprintln!("update check: the updater is misconfigured: {error}");
                return;
            }
        };

        match updater.check().await {
            // The signature on the manifest has been verified by this point.
            // An unsigned or tampered manifest arrives here as `Err`, not as
            // `Ok(None)` — see the tests at the bottom of this file.
            Ok(Some(update)) => {
                let version = update.version.clone();
                match app.state::<PendingUpdate>().0.lock() {
                    Ok(mut guard) => {
                        *guard = Some(update);
                        println!("update check: {version} is available.");
                    }
                    Err(_) => eprintln!("update check: the pending-update lock is poisoned"),
                }
            }
            Ok(None) => println!("update check: already on the newest version."),
            // No network, no release yet, or a manifest that failed
            // verification. None of those is the user's problem at launch.
            Err(error) => eprintln!("update check: {error}"),
        }
    });
}

#[cfg(test)]
// `expect` is denied in shell code because a panic takes the window with it.
// In a test it is the assertion. Same convention as `blinkify-engine`.
#[allow(clippy::expect_used)]
mod tests {
    //! The signature gate.
    //!
    //! Auto-update's entire security model is one signature check: without it,
    //! anyone who can serve a manifest can serve an executable to every
    //! installed copy. So the check is tested rather than assumed.
    //!
    //! None of this needs the production private key. A throwaway keypair is
    //! generated in the test, which is what makes a *positive* case possible at
    //! all — and a positive case is what stops these tests from passing because
    //! verification rejects everything.

    use minisign_verify::{PublicKey, Signature};

    /// Blinkify's real public key, read from the shipped configuration rather
    /// than pasted here. A copy would keep passing after the configured key was
    /// changed, which is the one moment this test needs to speak up.
    fn configured_public_key() -> String {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");

        // `.get` rather than indexing: `indexing_slicing` is denied across the
        // workspace because a length that came out of a file must never reach
        // an index unchecked, and a test is not an exception worth carving out.
        config
            .get("plugins")
            .and_then(|plugins| plugins.get("updater"))
            .and_then(|updater| updater.get("pubkey"))
            .and_then(serde_json::Value::as_str)
            .expect("plugins.updater.pubkey")
            .to_owned()
    }

    /// Tauri stores the public key base64-encoded around the minisign format.
    fn decode(pubkey: &str) -> String {
        use base64::Engine as _;
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(pubkey)
                .expect("base64"),
        )
        .expect("utf-8")
    }

    fn throwaway_keypair() -> (minisign::KeyPair, String) {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
        let public = pair.pk.to_box().expect("public key").to_string();
        (pair, public)
    }

    fn sign(pair: &minisign::KeyPair, payload: &[u8]) -> String {
        let signature = minisign::sign(None, &pair.sk, std::io::Cursor::new(payload), None, None)
            .expect("sign");
        signature.to_string()
    }

    #[test]
    fn the_configured_public_key_is_a_well_formed_minisign_key() {
        // A malformed key does not fail the build. It fails at runtime, on a
        // user's machine, as an update that never arrives.
        let key = decode(&configured_public_key());
        assert!(
            PublicKey::decode(&key).is_ok(),
            "plugins.updater.pubkey is not a minisign public key"
        );
    }

    #[test]
    fn a_genuine_signature_verifies() {
        // The control. Without it, a verifier that rejected everything would
        // pass every negative test below and ship an application that can never
        // update.
        let (pair, public) = throwaway_keypair();
        let payload = b"blinkify 0.2.0 installer bytes";
        let signature = sign(&pair, payload);

        let key = PublicKey::decode(&public).expect("public key");
        let signature = Signature::decode(&signature).expect("signature");

        assert!(key.verify(payload, &signature, false).is_ok());
    }

    #[test]
    fn a_tampered_payload_is_rejected() {
        // The case that matters: the signature is genuine, the bytes are not.
        let (pair, public) = throwaway_keypair();
        let signature = sign(&pair, b"blinkify 0.2.0 installer bytes");

        let key = PublicKey::decode(&public).expect("public key");
        let signature = Signature::decode(&signature).expect("signature");

        assert!(
            key.verify(b"blinkify 0.2.0 installer bytez", &signature, false)
                .is_err(),
            "a tampered payload verified against a genuine signature"
        );
    }

    #[test]
    fn a_signature_from_another_key_is_rejected_by_blinkify_s_key() {
        // Somebody else's perfectly valid signature is still not ours. This is
        // the attack the public key exists to stop: a manifest that verifies
        // against *a* key is worthless unless it verifies against *this* one.
        let (pair, _) = throwaway_keypair();
        let payload = b"blinkify 0.2.0 installer bytes";
        let signature = sign(&pair, payload);

        let ours = decode(&configured_public_key());
        let ours = PublicKey::decode(&ours).expect("public key");
        let signature = Signature::decode(&signature).expect("signature");

        assert!(
            ours.verify(payload, &signature, false).is_err(),
            "a signature made with a foreign key verified against Blinkify's key"
        );
    }

    #[test]
    fn a_garbage_signature_does_not_parse() {
        assert!(Signature::decode("not a signature").is_err());
        assert!(Signature::decode("").is_err());
    }
}
