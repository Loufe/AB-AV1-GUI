//! Facts used to order History independently of its storage engine.

use serde::Deserialize;

use crate::{Observation, ObservedOutcome, TranslatedOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum HistoryOutcome {
    Converted,
    Remuxed,
    NotWorthwhile,
    Analyzed,
    Failed,
    Stopped,
    Incomplete,
}

impl HistoryOutcome {
    #[must_use]
    pub fn of(observation: &Observation) -> Self {
        match observation {
            Observation::Native(native) => match native.outcome {
                ObservedOutcome::Analyzed { .. } => Self::Analyzed,
                ObservedOutcome::Converted { .. } => Self::Converted,
                ObservedOutcome::Remuxed { .. } => Self::Remuxed,
                ObservedOutcome::NotWorthwhile { .. } => Self::NotWorthwhile,
                ObservedOutcome::Failed { .. } => Self::Failed,
                ObservedOutcome::Stopped => Self::Stopped,
                ObservedOutcome::Incomplete => Self::Incomplete,
            },
            Observation::Translated(translated) => match translated.outcome {
                TranslatedOutcome::Analyzed { .. } => Self::Analyzed,
                TranslatedOutcome::Converted { .. } => Self::Converted,
                TranslatedOutcome::NotWorthwhile { .. } => Self::NotWorthwhile,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryBrowseFacts {
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub change: Option<i128>,
    pub quality: Option<u64>,
    pub took: Option<u64>,
}

impl HistoryBrowseFacts {
    #[must_use]
    pub fn of(observation: &Observation) -> Self {
        let (measured, inspected, quality, took, output) = match observation {
            Observation::Native(native) => {
                let measured = match &native.outcome {
                    ObservedOutcome::Converted { encode, .. } => encode.measurement.sizes(),
                    ObservedOutcome::Remuxed { remux } => remux.measurement.sizes(),
                    _ => None,
                }
                .map(|sizes| (sizes.input, sizes.output));
                let quality = match &native.outcome {
                    ObservedOutcome::Analyzed { search }
                    | ObservedOutcome::Converted { search, .. } => {
                        Some(u64::from(search.analysis.measurement.crf.0))
                    }
                    _ => None,
                };
                let took = match &native.outcome {
                    ObservedOutcome::Converted { encode, .. } => {
                        encode.duration.map(|duration| duration.0)
                    }
                    _ => None,
                };
                (
                    measured,
                    native.source.as_ref().map(|source| source.size_bytes),
                    quality,
                    took,
                    None,
                )
            }
            Observation::Translated(translated) => {
                let (quality, output, took) = match &translated.outcome {
                    TranslatedOutcome::Converted {
                        quality,
                        output_size,
                        encode_duration,
                    } => (
                        quality.crf.map(|crf| u64::from(crf.0)),
                        *output_size,
                        encode_duration.map(|duration| duration.0),
                    ),
                    TranslatedOutcome::Analyzed { quality } => {
                        (quality.crf.map(|crf| u64::from(crf.0)), None, None)
                    }
                    TranslatedOutcome::NotWorthwhile { .. } => (None, None, None),
                };
                (
                    translated.source.size_bytes.zip(output),
                    translated.source.size_bytes,
                    quality,
                    took,
                    output,
                )
            }
        };
        Self {
            before: measured.map(|(before, _)| before).or(inspected),
            after: measured.map(|(_, after)| after).or(output),
            change: measured
                .filter(|(before, _)| *before > 0)
                .map(|(before, after)| {
                    (i128::from(before) - i128::from(after)) * 10_000 / i128::from(before)
                }),
            quality,
            took,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ImportOrigin, RecordKey, TranslatedObservation, TranslatedQuality, TranslatedSource,
    };

    fn converted(before: Option<u64>, after: Option<u64>) -> Observation {
        Observation::Translated(TranslatedObservation {
            origin: ImportOrigin::V2History,
            record_key: RecordKey::new("0123456789abcdef".to_owned())
                .unwrap_or_else(|error| panic!("test key: {error}")),
            source: TranslatedSource {
                codec: None,
                width: None,
                height: None,
                duration_ms: None,
                size_bytes: before,
            },
            outcome: TranslatedOutcome::Converted {
                quality: TranslatedQuality {
                    crf: None,
                    score: None,
                    target: None,
                },
                output_size: after,
                encode_duration: None,
            },
            updated_at: None,
        })
    }

    #[test]
    fn change_preserves_full_range_and_truncates_toward_zero() {
        assert_eq!(
            HistoryBrowseFacts::of(&converted(Some(1), Some(u64::MAX))).change,
            Some(-184_467_440_737_095_516_140_000)
        );
        assert_eq!(
            HistoryBrowseFacts::of(&converted(Some(3), Some(4))).change,
            Some(-3333)
        );
        assert_eq!(
            HistoryBrowseFacts::of(&converted(Some(3), Some(2))).change,
            Some(3333)
        );
        assert_eq!(
            HistoryBrowseFacts::of(&converted(Some(0), Some(1))).change,
            None
        );
        assert_eq!(
            HistoryBrowseFacts::of(&converted(None, Some(1))).after,
            Some(1)
        );
        assert_eq!(
            HistoryBrowseFacts::of(&converted(Some(1), None)).change,
            None
        );
    }

    #[test]
    fn measured_input_takes_precedence_over_inspected_size_and_sparse_recovery_stays_absent()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut native: crate::NativeObservation = serde_json::from_str(include_str!(
            "../tests/fixtures/observations/native/valid/remuxed_live.json"
        ))?;
        if let Some(source) = &mut native.source {
            source.size_bytes = u64::MAX;
        }
        let observation = Observation::Native(Box::new(native));
        let facts = HistoryBrowseFacts::of(&observation);
        let Observation::Native(native) = &observation else {
            return Err("expected native".into());
        };
        let ObservedOutcome::Remuxed { remux } = &native.outcome else {
            return Err("expected remux".into());
        };
        assert_eq!(
            facts.before,
            remux.measurement.sizes().map(|sizes| sizes.input)
        );
        assert_eq!(facts.quality, None);
        assert_eq!(facts.took, None);
        let recovered: crate::NativeObservation = serde_json::from_str(include_str!(
            "../tests/fixtures/observations/native/valid/converted_recovered_without_sizes.json"
        ))?;
        let inspected = recovered.source.as_ref().map(|source| source.size_bytes);
        let facts = HistoryBrowseFacts::of(&Observation::Native(Box::new(recovered)));
        assert_eq!(facts.before, inspected);
        assert_eq!((facts.after, facts.change, facts.took), (None, None, None));
        Ok(())
    }
}
