//! Spec fixtures for the History observation contract (ADR-024, ADR-025).
//! Each JSON file under `tests/fixtures/observations/native/` is one
//! hand-maintained `NativeObservation`, and each under `translated/` one
//! `TranslatedObservation`:
//!
//! - `valid/`: deserializes and validates.
//! - `invalid/`: deserializes but `validate` rejects it with the message in
//!   `rejects_with`.
//! - `unrepresentable/`: the type shape refuses to deserialize it at all.
//!
//! Adding an observation fact or rule means adding the fixture that proves
//! it.
#![forbid(unsafe_code)]

use std::{fmt::Debug, fs, path::Path};

use crfty_core::{NativeObservation, TranslatedObservation};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/observations");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rejected<T> {
    observation: T,
    rejects_with: String,
}

type Validate<T> = fn(&T) -> Result<(), &'static str>;

fn fixtures(family: &str, kind: &str) -> Vec<(String, String)> {
    let directory = Path::new(FIXTURES).join(family).join(kind);
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

fn assert_valid<T>(family: &str, validate: Validate<T>)
where
    T: DeserializeOwned + Serialize + PartialEq + Debug,
{
    for (name, text) in fixtures(family, "valid") {
        let observation: T = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{name}: deserialize: {error}"));
        assert_eq!(validate(&observation), Ok(()), "{name}");
        let encoded = serde_json::to_string(&observation)
            .unwrap_or_else(|error| panic!("{name}: serialize: {error}"));
        let decoded: T = serde_json::from_str(&encoded)
            .unwrap_or_else(|error| panic!("{name}: round trip: {error}"));
        assert_eq!(decoded, observation, "{name}");
    }
}

fn assert_invalid<T>(family: &str, validate: Validate<T>)
where
    T: DeserializeOwned,
{
    for (name, text) in fixtures(family, "invalid") {
        let rejected: Rejected<T> = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{name}: deserialize: {error}"));
        assert_eq!(
            validate(&rejected.observation),
            Err(rejected.rejects_with.as_str()),
            "{name}"
        );
    }
}

fn assert_unrepresentable<T>(family: &str)
where
    T: DeserializeOwned,
{
    for (name, text) in fixtures(family, "unrepresentable") {
        assert!(
            serde_json::from_str::<T>(&text).is_err(),
            "{name}: the type shape must refuse this"
        );
    }
}

#[test]
fn valid_native_fixtures_deserialize_and_validate() {
    assert_valid::<NativeObservation>("native", NativeObservation::validate);
}

#[test]
fn invalid_native_fixtures_are_rejected_with_the_recorded_reason() {
    assert_invalid::<NativeObservation>("native", NativeObservation::validate);
}

#[test]
fn unrepresentable_native_fixtures_do_not_deserialize() {
    assert_unrepresentable::<NativeObservation>("native");
}

#[test]
fn valid_translated_fixtures_deserialize_and_validate() {
    assert_valid::<TranslatedObservation>("translated", TranslatedObservation::validate);
}

#[test]
fn invalid_translated_fixtures_are_rejected_with_the_recorded_reason() {
    assert_invalid::<TranslatedObservation>("translated", TranslatedObservation::validate);
}

#[test]
fn unrepresentable_translated_fixtures_do_not_deserialize() {
    assert_unrepresentable::<TranslatedObservation>("translated");
}
