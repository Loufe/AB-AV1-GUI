//! Wire-safe time newtypes. Two clocks, never nanoseconds on the wire:
//! instants are wall-clock milliseconds stamped by the engine, durations are
//! monotonic measurements shipped as milliseconds. Filesystem modification
//! times keep nanosecond precision but cross serde as strings because
//! epoch-nanoseconds exceed JavaScript's exact-integer range.

use serde::{Deserialize, Serialize};

/// A wall-clock instant in milliseconds since the Unix epoch. Stamped by the
/// engine (core has no clock) and delivered inside command payloads.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
pub struct UnixMillis(#[specta(type = crate::JsNumber)] pub u64);

/// A duration in milliseconds, measured engine-side with a monotonic clock.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
pub struct DurationMs(#[specta(type = crate::JsNumber)] pub u64);

/// A filesystem modification time in nanoseconds since the Unix epoch.
/// Serialized as a JSON string: the value routinely exceeds 2^53, which a
/// JavaScript `number` would silently round. The frontend treats it as opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
pub struct FileTimeNs(#[specta(type = String)] pub u64);

impl Serialize for FileTimeNs {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for FileTimeNs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse::<u64>()
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::{DurationMs, FileTimeNs, UnixMillis};

    #[test]
    fn file_time_round_trips_through_a_json_string() {
        // Above 2^53: survives only because the wire representation is text.
        let original = FileTimeNs(1_752_871_234_567_890_123);
        let encoded = serde_json::to_string(&original).expect("serialize file time");
        assert_eq!(encoded, "\"1752871234567890123\"");
        let decoded: FileTimeNs = serde_json::from_str(&encoded).expect("deserialize file time");
        assert_eq!(decoded, original);
    }

    #[test]
    fn file_time_rejects_non_numeric_text() {
        assert!(serde_json::from_str::<FileTimeNs>("\"not-a-number\"").is_err());
        assert!(serde_json::from_str::<FileTimeNs>("12345").is_err());
    }

    #[test]
    fn millisecond_newtypes_stay_plain_numbers_on_the_wire() {
        assert_eq!(
            serde_json::to_string(&UnixMillis(1_752_871_234_567)).expect("serialize instant"),
            "1752871234567"
        );
        assert_eq!(
            serde_json::to_string(&DurationMs(65_000)).expect("serialize duration"),
            "65000"
        );
    }
}
