//! Spec fixtures for the History observation contract (ADR-024). Each JSON
//! file under `tests/fixtures/observations/` is one hand-maintained
//! observation:
//!
//! - `valid/`: deserializes and validates.
//! - `invalid/`: deserializes but `validate` rejects it with the message in
//!   `rejects_with`.
//! - `unrepresentable/`: the type shape refuses to deserialize it at all.
//!
//! Adding an observation fact or rule means adding the fixture that proves
//! it.
#![forbid(unsafe_code)]

use std::{fs, path::Path};

use crfty_core::Observation;
use serde::Deserialize;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/observations");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rejected {
    observation: Observation,
    rejects_with: String,
}

fn fixtures(kind: &str) -> Vec<(String, String)> {
    let directory = Path::new(FIXTURES).join(kind);
    let mut found: Vec<(String, String)> = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        .map(|entry| {
            let path = entry
                .unwrap_or_else(|error| panic!("read entry in {}: {error}", directory.display()))
                .path();
            let name = path.display().to_string();
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            (name, text)
        })
        .collect();
    found.sort();
    assert!(
        !found.is_empty(),
        "no fixtures under {}",
        directory.display()
    );
    found
}

#[test]
fn valid_fixtures_deserialize_and_validate() {
    for (name, text) in fixtures("valid") {
        let observation: Observation = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{name}: deserialize: {error}"));
        assert_eq!(observation.validate(), Ok(()), "{name}");
        let encoded = serde_json::to_string(&observation)
            .unwrap_or_else(|error| panic!("{name}: serialize: {error}"));
        let decoded: Observation = serde_json::from_str(&encoded)
            .unwrap_or_else(|error| panic!("{name}: round trip: {error}"));
        assert_eq!(decoded, observation, "{name}");
    }
}

#[test]
fn invalid_fixtures_are_rejected_with_the_recorded_reason() {
    for (name, text) in fixtures("invalid") {
        let rejected: Rejected = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{name}: deserialize: {error}"));
        assert_eq!(
            rejected.observation.validate(),
            Err(rejected.rejects_with.as_str()),
            "{name}"
        );
    }
}

#[test]
fn unrepresentable_fixtures_do_not_deserialize() {
    for (name, text) in fixtures("unrepresentable") {
        assert!(
            serde_json::from_str::<Observation>(&text).is_err(),
            "{name}: the type shape must refuse this"
        );
    }
}
