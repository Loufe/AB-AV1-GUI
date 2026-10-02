use std::{collections::BTreeMap, sync::LazyLock};

use serde_json::{Map, Value as Json};

use crfty_core::Observation;

use super::{Result, StoreError, database::Value, number};

pub(super) type Facts = BTreeMap<String, Value>;
pub(super) type Children = BTreeMap<String, Vec<Facts>>;

enum Shape {
    Number,
    Text,
    Bool,
    Unit,
    Optional(Box<Self>),
    Object(Vec<(&'static str, Self)>),
    Enum(Vec<(&'static str, Self)>),
    Attempts,
}

macro_rules! object {
    ($($field:literal: $shape:expr),* $(,)?) => {
        Shape::Object(vec![$(($field, $shape)),*])
    };
}

macro_rules! variants {
    ($($variant:literal: $shape:expr),* $(,)?) => {
        Shape::Enum(vec![$(($variant, $shape)),*])
    };
}

fn optional(shape: Shape) -> Shape {
    Shape::Optional(Box::new(shape))
}

fn codec() -> Shape {
    variants!("Av1": Shape::Unit, "H264": Shape::Unit, "Hevc": Shape::Unit, "Vp9": Shape::Unit, "Other": Shape::Text)
}

fn assessment() -> Shape {
    optional(
        variants!("Matched": Shape::Unit, "IdentityDiffers": Shape::Unit, "PropertiesChanged": Shape::Unit, "Absent": Shape::Unit, "Unassessable": Shape::Unit),
    )
}

fn decode() -> Shape {
    variants!("Software": Shape::Unit, "Hardware": variants!(
        "H264Cuvid": Shape::Unit, "H264Qsv": Shape::Unit, "HevcCuvid": Shape::Unit,
        "HevcQsv": Shape::Unit, "Vp9Cuvid": Shape::Unit, "Vp9Qsv": Shape::Unit,
        "Av1Cuvid": Shape::Unit, "Av1Qsv": Shape::Unit
    ))
}

fn measurement() -> Shape {
    object!("crf": Shape::Number, "score": Shape::Number, "predicted_size": Shape::Number,
        "predicted_percent_basis_points": Shape::Number, "predicted_duration_ms": Shape::Number,
        "from_cache": Shape::Bool)
}

fn profile() -> Shape {
    object!("preset": Shape::Number, "max_encoded_percent_basis_points": Shape::Number,
        "samples": optional(Shape::Number), "sample_duration_ms": Shape::Number,
        "thorough": Shape::Bool, "decode_mode": decode(), "ab_av1_revision": Shape::Text,
        "ffmpeg_revision": Shape::Text, "encoder_revision": Shape::Text)
}

fn search() -> Shape {
    object!("analysis": object!(
        "requested_target": Shape::Number, "successful_target": Shape::Number,
        "fallback_floor": Shape::Number, "fallback_step": Shape::Number,
        "failed_attempts": Shape::Attempts, "measurement": measurement(), "profile": profile()
    ), "duration": optional(Shape::Number), "assessment": assessment())
}

fn sizes() -> Shape {
    object!("input": Shape::Number, "output": Shape::Number)
}

fn quality() -> Shape {
    object!("crf": optional(Shape::Number), "score": optional(Shape::Number), "target": optional(Shape::Number))
}

static ATTEMPT: LazyLock<Shape> =
    LazyLock::new(|| object!("target": Shape::Number, "last_measurement": optional(measurement())));

static OBSERVATION: LazyLock<Shape> = LazyLock::new(|| {
    variants!(
        "Native": object!(
            "run_id": Shape::Number,
            "operation": variants!("Analyze": Shape::Unit, "Convert": Shape::Unit),
            "source": optional(object!("content_key": Shape::Text, "codec": codec(), "width": Shape::Number,
                "height": Shape::Number, "duration_ms": Shape::Number, "size_bytes": Shape::Number)),
            "revisions": object!("ab_av1": Shape::Text, "ffmpeg": Shape::Text, "encoder": Shape::Text),
            "outcome": variants!(
                "Analyzed": object!("search": search()),
                "Converted": object!("search": search(), "encode": object!(
                    "measurement": variants!("Live": object!("sizes": sizes(), "decode": decode()),
                        "Recovered": object!("sizes": optional(sizes()))),
                    "duration": optional(Shape::Number), "output_content_key": optional(Shape::Text), "assessment": assessment())),
                "Remuxed": object!("remux": object!(
                    "measurement": variants!("Live": object!("sizes": sizes()), "Recovered": object!("sizes": optional(sizes()))),
                    "output_content_key": optional(Shape::Text), "assessment": assessment())),
                "NotWorthwhile": object!("requested": Shape::Number, "floor": Shape::Number, "attempts": Shape::Attempts),
                "Failed": object!("facts": object!("kind": variants!(
                    "SearchStart": Shape::Unit, "SearchRun": Shape::Unit, "EncodeStart": Shape::Unit,
                    "EncodeRun": Shape::Unit, "RemuxStart": Shape::Unit, "RemuxRun": Shape::Unit,
                    "AdapterPanicked": object!("cleanup_failed": Shape::Bool), "OutputPrepare": Shape::Unit,
                    "OutputPromote": Shape::Unit, "OutputConflict": Shape::Unit, "Internal": Shape::Unit
                ), "message": Shape::Text, "diagnostic": Shape::Text)),
                "Stopped": Shape::Unit, "Incomplete": Shape::Unit
            ),
            "started_at": optional(Shape::Number), "finished_at": optional(Shape::Number)
        ),
        "Translated": object!(
            "origin": variants!("V2History": Shape::Unit), "record_key": Shape::Text,
            "source": object!("codec": optional(codec()), "width": optional(Shape::Number),
                "height": optional(Shape::Number), "duration_ms": optional(Shape::Number), "size_bytes": optional(Shape::Number)),
            "outcome": variants!("Analyzed": object!("quality": quality()),
                "Converted": object!("quality": quality(), "output_size": optional(Shape::Number), "encode_duration": optional(Shape::Number)),
                "NotWorthwhile": object!("requested": optional(Shape::Number), "floor": optional(Shape::Number))),
            "updated_at": optional(Shape::Number)
        )
    )
});

pub(super) struct Packed {
    pub(super) facts: Facts,
    pub(super) children: Children,
}

pub(super) fn pack(observation: &Observation) -> Result<Packed> {
    observation
        .validate()
        .map_err(|error| StoreError::context("invalid History observation", error))?;
    let json = serde_json::to_value(observation)
        .map_err(|error| StoreError::context("map History facts", error))?;
    let mut packed = Packed {
        facts: Facts::new(),
        children: Children::new(),
    };
    OBSERVATION.write("fact", &json, &mut packed.facts, &mut packed.children)?;
    Ok(packed)
}

pub(super) fn unpack(facts: &Facts, children: &Children) -> Result<Observation> {
    let json = OBSERVATION.read("fact", facts, children)?;
    let observation: Observation = serde_json::from_value(json)
        .map_err(|error| StoreError::context("decode relational History facts", error))?;
    observation
        .validate()
        .map_err(|error| StoreError::context("invalid stored History shape", error))?;
    let expected = pack(&observation)?;
    let expected_lists: std::collections::BTreeSet<_> = expected
        .children
        .iter()
        .filter(|(_, entries)| !entries.is_empty())
        .map(|(list, _)| list)
        .collect();
    let stored_lists: std::collections::BTreeSet<_> = children
        .iter()
        .filter(|(_, entries)| !entries.is_empty())
        .map(|(list, _)| list)
        .collect();
    if stored_lists != expected_lists {
        return Err(StoreError(
            "History child evidence disagrees with its observation shape".to_owned(),
        ));
    }
    Ok(observation)
}

pub(super) fn columns(attempt: bool) -> BTreeMap<String, &'static str> {
    let mut columns = BTreeMap::new();
    if attempt {
        ATTEMPT.columns("attempt", &mut columns);
    } else {
        OBSERVATION.columns("fact", &mut columns);
    }
    columns
}

pub(super) fn shape_checks(attempt: bool) -> String {
    let mut checks = Vec::new();
    if attempt {
        ATTEMPT.checks("attempt", "1", &mut checks);
    } else {
        OBSERVATION.checks("fact", "1", &mut checks);
    }
    checks.join(",")
}

impl Shape {
    fn write(
        &self,
        name: &str,
        json: &Json,
        facts: &mut Facts,
        children: &mut Children,
    ) -> Result<()> {
        let invalid = || StoreError(format!("History mapping shape mismatch at {name}"));
        match self {
            Self::Number => {
                facts.insert(
                    name.to_owned(),
                    Value::Blob(number::unsigned(json.as_u64().ok_or_else(invalid)?)),
                );
            }
            Self::Text => {
                facts.insert(
                    name.to_owned(),
                    Value::Text(json.as_str().ok_or_else(invalid)?.to_owned()),
                );
            }
            Self::Bool => {
                facts.insert(
                    name.to_owned(),
                    Value::Integer(i64::from(json.as_bool().ok_or_else(invalid)?)),
                );
            }
            Self::Unit => {
                if !json.is_null() {
                    return Err(invalid());
                }
            }
            Self::Optional(shape) => {
                facts.insert(
                    format!("{name}_present"),
                    Value::Integer(i64::from(!json.is_null())),
                );
                if !json.is_null() {
                    shape.write(name, json, facts, children)?;
                }
            }
            Self::Object(fields) => {
                let object = json.as_object().ok_or_else(invalid)?;
                if object.len() != fields.len() {
                    return Err(invalid());
                }
                for (field, shape) in fields {
                    shape.write(
                        &format!("{name}_{field}"),
                        object.get(*field).ok_or_else(invalid)?,
                        facts,
                        children,
                    )?;
                }
            }
            Self::Enum(variants) => {
                let (tag, body) = if let Some(tag) = json.as_str() {
                    (tag, &Json::Null)
                } else {
                    let object = json
                        .as_object()
                        .filter(|object| object.len() == 1)
                        .ok_or_else(invalid)?;
                    let (tag, body) = object.iter().next().ok_or_else(invalid)?;
                    (tag.as_str(), body)
                };
                let (_, shape) = variants
                    .iter()
                    .find(|(variant, _)| *variant == tag)
                    .ok_or_else(invalid)?;
                facts.insert(format!("{name}_tag"), Value::Text(tag.to_owned()));
                shape.write(
                    &format!("{name}_{}", tag.to_ascii_lowercase()),
                    body,
                    facts,
                    children,
                )?;
            }
            Self::Attempts => {
                let array = json.as_array().ok_or_else(invalid)?;
                let mut entries = Vec::new();
                for json in array {
                    let mut entry = Facts::new();
                    ATTEMPT.write("attempt", json, &mut entry, children)?;
                    entries.push(entry);
                }
                children.insert(name.to_owned(), entries);
            }
        }
        Ok(())
    }

    fn read(&self, name: &str, facts: &Facts, children: &Children) -> Result<Json> {
        let invalid = || StoreError(format!("stored History shape mismatch at {name}"));
        match self {
            Self::Number => match facts.get(name) {
                Some(Value::Blob(bytes)) => Ok(Json::from(number::read_unsigned(bytes)?)),
                _ => Err(invalid()),
            },
            Self::Text => match facts.get(name) {
                Some(Value::Text(text)) => Ok(Json::String(text.clone())),
                _ => Err(invalid()),
            },
            Self::Bool => match facts.get(name) {
                Some(Value::Integer(value)) if *value == 0 || *value == 1 => {
                    Ok(Json::Bool(*value == 1))
                }
                _ => Err(invalid()),
            },
            Self::Unit => Ok(Json::Null),
            Self::Optional(shape) => match facts.get(&format!("{name}_present")) {
                Some(Value::Integer(0)) => Ok(Json::Null),
                Some(Value::Integer(1)) => shape.read(name, facts, children),
                _ => Err(invalid()),
            },
            Self::Object(fields) => {
                let mut object = Map::new();
                for (field, shape) in fields {
                    object.insert(
                        (*field).to_owned(),
                        shape.read(&format!("{name}_{field}"), facts, children)?,
                    );
                }
                Ok(Json::Object(object))
            }
            Self::Enum(variants) => {
                let Some(Value::Text(tag)) = facts.get(&format!("{name}_tag")) else {
                    return Err(invalid());
                };
                let (_, shape) = variants
                    .iter()
                    .find(|(variant, _)| *variant == tag)
                    .ok_or_else(invalid)?;
                if matches!(shape, Self::Unit) {
                    return Ok(Json::String(tag.clone()));
                }
                let body = shape.read(
                    &format!("{name}_{}", tag.to_ascii_lowercase()),
                    facts,
                    children,
                )?;
                Ok(Json::Object(Map::from_iter([(tag.clone(), body)])))
            }
            Self::Attempts => {
                let entries = children.get(name).map(Vec::as_slice).unwrap_or_default();
                entries
                    .iter()
                    .map(|entry| ATTEMPT.read("attempt", entry, children))
                    .collect::<Result<Vec<_>>>()
                    .map(Json::Array)
            }
        }
    }

    fn columns(&self, name: &str, columns: &mut BTreeMap<String, &'static str>) {
        match self {
            Self::Number => {
                columns.insert(name.to_owned(), "BLOB");
            }
            Self::Text => {
                columns.insert(name.to_owned(), "TEXT");
            }
            Self::Bool => {
                columns.insert(name.to_owned(), "INTEGER");
            }
            Self::Optional(shape) => {
                columns.insert(format!("{name}_present"), "INTEGER");
                shape.columns(name, columns);
            }
            Self::Object(fields) => {
                for (field, shape) in fields {
                    shape.columns(&format!("{name}_{field}"), columns);
                }
            }
            Self::Enum(variants) => {
                columns.insert(format!("{name}_tag"), "TEXT");
                for (tag, shape) in variants {
                    shape.columns(&format!("{name}_{}", tag.to_ascii_lowercase()), columns);
                }
            }
            Self::Unit | Self::Attempts => {}
        }
    }

    fn checks(&self, name: &str, guard: &str, checks: &mut Vec<String>) {
        let leaf = |name: &str, valid: String| {
            format!("CHECK(COALESCE(CASE WHEN {guard} THEN ({valid}) ELSE {name} IS NULL END,0))")
        };
        match self {
            Self::Number => checks.push(leaf(
                name,
                format!("typeof({name})='blob' AND length({name})=8"),
            )),
            Self::Text => checks.push(leaf(name, format!("typeof({name})='text'"))),
            Self::Bool => checks.push(leaf(
                name,
                format!("typeof({name})='integer' AND {name} IN (0,1)"),
            )),
            Self::Unit | Self::Attempts => {}
            Self::Optional(shape) => {
                let marker = format!("{name}_present");
                Self::Bool.checks(&marker, guard, checks);
                shape.checks(name, &format!("({guard}) AND {marker} IS 1"), checks);
            }
            Self::Object(fields) => {
                for (field, shape) in fields {
                    shape.checks(&format!("{name}_{field}"), guard, checks);
                }
            }
            Self::Enum(variants) => {
                let discriminator = format!("{name}_tag");
                let tags = variants
                    .iter()
                    .map(|(tag, _)| format!("'{tag}'"))
                    .collect::<Vec<_>>()
                    .join(",");
                checks.push(leaf(&discriminator, format!("{discriminator} IN ({tags})")));
                for (tag, shape) in variants {
                    shape.checks(
                        &format!("{name}_{}", tag.to_ascii_lowercase()),
                        &format!("({guard}) AND {discriminator} IS '{tag}'"),
                        checks,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_translated_facts_round_trip_with_all_unsigned_bits() -> Result<()> {
        use crfty_core::{
            ImportOrigin, RecordKey, TranslatedObservation, TranslatedOutcome, TranslatedQuality,
            TranslatedSource, UnixMillis,
        };
        let observation = Observation::Translated(TranslatedObservation {
            origin: ImportOrigin::V2History,
            record_key: RecordKey::new("0123456789abcdef".to_owned())
                .map_err(|error| StoreError::context("record key", error))?,
            source: TranslatedSource {
                codec: Some(crfty_core::VideoCodec::Other("custom".to_owned())),
                width: None,
                height: None,
                duration_ms: Some(u64::MAX),
                size_bytes: Some(1_u64 << 63),
            },
            outcome: TranslatedOutcome::Analyzed {
                quality: TranslatedQuality {
                    crf: None,
                    score: None,
                    target: None,
                },
            },
            updated_at: Some(UnixMillis(u64::MAX)),
        });
        let packed = pack(&observation)?;
        assert_eq!(unpack(&packed.facts, &packed.children)?, observation);
        Ok(())
    }
}
