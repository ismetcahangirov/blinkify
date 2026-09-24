//! Schema migration: every project file a released build ever wrote opens in
//! every later one.
//!
//! A migration takes the JSON of one schema version and returns the JSON of
//! the next. They work on [`serde_json::Value`], not on the Rust types,
//! because the types only describe the current schema — an old file has to
//! be read as what it was. [`to_current`] runs them in order from the file's
//! version to [`SCHEMA_VERSION`].
//!
//! To change the schema: bump [`SCHEMA_VERSION`], append the migration from
//! the previous version to [`MIGRATIONS`], and commit a file written by the
//! previous version to `tests/fixtures/projects/v<N>.blinkify`. The test
//! suite opens every fixture, and fails if a version has none.

use serde_json::Value;

use super::{ProjectError, SCHEMA_VERSION};

/// One step: the JSON of version `from`, as the JSON of version `from + 1`.
type Migration = fn(Value) -> Result<Value, ProjectError>;

/// `MIGRATIONS[n]` migrates version `n + 1` to `n + 2`.
const MIGRATIONS: &[Migration] = &[v1_to_v2, v2_to_v3];

/// Schema 2 (#57): the sequence records whether its settings still wait for
/// the first clip. A version-1 sequence with no clip had never been given
/// settings — they were the placeholder default — so it waits; one with clips
/// keeps the settings it was edited at.
fn v1_to_v2(mut value: Value) -> Result<Value, ProjectError> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| ProjectError::Corrupt("the project is not an object".to_owned()))?;
    let sequence = object
        .get_mut("sequence")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ProjectError::Corrupt("no sequence".to_owned()))?;
    let empty = sequence
        .get("tracks")
        .and_then(Value::as_array)
        .is_none_or(|tracks| {
            tracks.iter().all(|track| {
                track
                    .get("clips")
                    .and_then(Value::as_array)
                    .is_none_or(Vec::is_empty)
            })
        });
    sequence.insert("matchFirstClip".to_owned(), Value::Bool(empty));
    object.insert("schemaVersion".to_owned(), Value::from(2));
    Ok(value)
}

/// The version a project file declares.
///
/// # Errors
///
/// The file has no `schemaVersion`, or not a positive integer.
pub fn version_of(value: &Value) -> Result<u32, ProjectError> {
    value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .filter(|&version| version >= 1)
        .ok_or_else(|| ProjectError::Corrupt("no schemaVersion".to_owned()))
}

/// Bring a project file's JSON to the current schema.
///
/// # Errors
///
/// No version, a version newer than this build, or a migration that failed.
pub fn to_current(value: Value) -> Result<Value, ProjectError> {
    to(value, SCHEMA_VERSION, MIGRATIONS)
}

fn to(mut value: Value, target: u32, migrations: &[Migration]) -> Result<Value, ProjectError> {
    let found = version_of(&value)?;
    if found > target {
        return Err(ProjectError::FutureVersion {
            found,
            supported: target,
        });
    }
    for version in found..target {
        let step = usize::try_from(version - 1)
            .ok()
            .and_then(|index| migrations.get(index))
            .ok_or_else(|| ProjectError::Invalid(format!("no migration from schema {version}")))?;
        value = step(value)?;
        if version_of(&value)? != version + 1 {
            return Err(ProjectError::Invalid(format!(
                "the migration from schema {version} did not produce schema {}",
                version + 1
            )));
        }
    }
    Ok(value)
}

/// Schema 3 (#35): two operations, `freeze` and `reverse`. A version-2
/// file has neither, so nothing in it changes but its version — which is
/// bumped at all so that a version-2 build refuses a file holding one with
/// "saved by a newer version", not "damaged".
fn v2_to_v3(mut value: Value) -> Result<Value, ProjectError> {
    value
        .as_object_mut()
        .ok_or_else(|| ProjectError::Corrupt("the project is not an object".to_owned()))?
        .insert("schemaVersion".to_owned(), Value::from(3));
    Ok(value)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unnecessary_wraps,
    clippy::indexing_slicing
)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_version_has_a_migration_to_the_next() {
        let expected = usize::try_from(SCHEMA_VERSION - 1).expect("small");
        assert_eq!(MIGRATIONS.len(), expected);
    }

    // The registry is exercised here with made-up versions, before a real
    // migration exists to exercise it.
    fn rename_title(mut value: Value) -> Result<Value, ProjectError> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| ProjectError::Corrupt("x".into()))?;
        if let Some(title) = object.remove("title") {
            object.insert("name".to_owned(), title);
        }
        object.insert("schemaVersion".to_owned(), json!(2));
        Ok(value)
    }

    fn add_tracks(mut value: Value) -> Result<Value, ProjectError> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| ProjectError::Corrupt("x".into()))?;
        object.insert("tracks".to_owned(), json!([]));
        object.insert("schemaVersion".to_owned(), json!(3));
        Ok(value)
    }

    fn forgets_the_version(value: Value) -> Result<Value, ProjectError> {
        Ok(value)
    }

    #[test]
    fn migrations_run_in_order_from_the_files_version() {
        let registry: &[Migration] = &[rename_title, add_tracks];
        let v1 = json!({ "schemaVersion": 1, "title": "Old" });
        assert_eq!(
            to(v1, 3, registry).expect("migrate"),
            json!({ "schemaVersion": 3, "name": "Old", "tracks": [] })
        );
        let v2 = json!({ "schemaVersion": 2, "name": "Newer" });
        assert_eq!(
            to(v2, 3, registry).expect("migrate"),
            json!({ "schemaVersion": 3, "name": "Newer", "tracks": [] })
        );
    }

    #[test]
    fn a_migration_must_advance_the_version() {
        let registry: &[Migration] = &[forgets_the_version];
        let error = to(json!({ "schemaVersion": 1 }), 2, registry).expect_err("refused");
        assert!(matches!(error, ProjectError::Invalid(_)));
    }

    #[test]
    fn schema_1_waits_for_a_first_clip_only_when_it_has_none() {
        let empty = json!({
            "schemaVersion": 1,
            "sequence": { "settings": {}, "tracks": [{ "clips": [] }] }
        });
        let migrated = v1_to_v2(empty).expect("migrate");
        assert_eq!(migrated["schemaVersion"], 2);
        assert_eq!(migrated["sequence"]["matchFirstClip"], true);
        let edited = json!({
            "schemaVersion": 1,
            "sequence": { "settings": {}, "tracks": [{ "clips": [{ "id": 1 }] }] }
        });
        assert_eq!(
            v1_to_v2(edited).expect("migrate")["sequence"]["matchFirstClip"],
            false
        );
        assert!(v1_to_v2(json!({ "schemaVersion": 1 })).is_err());
    }

    #[test]
    fn a_future_or_missing_version_is_refused() {
        assert_eq!(
            to_current(json!({ "schemaVersion": SCHEMA_VERSION + 1 })),
            Err(ProjectError::FutureVersion {
                found: SCHEMA_VERSION + 1,
                supported: SCHEMA_VERSION
            })
        );
        for broken in [
            json!({}),
            json!({ "schemaVersion": 0 }),
            json!({ "schemaVersion": "1" }),
        ] {
            assert!(matches!(to_current(broken), Err(ProjectError::Corrupt(_))));
        }
    }
}
