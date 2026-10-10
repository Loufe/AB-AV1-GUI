use super::{
    Result, StoreError,
    database::{Database, Value},
    mapping, number,
};

pub(super) fn initialize(database: &mut Database) -> Result<()> {
    let version = database.query("PRAGMA user_version", &[])?;
    match version.first().and_then(|row| row.first()) {
        Some(Value::Integer(1)) => return Ok(()),
        Some(Value::Integer(0)) => {}
        _ => return Err(StoreError("unsupported History schema".to_owned())),
    }
    let transaction = database.transaction(true)?;
    let database = transaction.database;
    let facts = columns(false);
    let attempts = columns(true);
    database.batch(&format!(
        "CREATE TABLE IF NOT EXISTS history_meta (
            singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
            recording_high_water BLOB NOT NULL CHECK(length(recording_high_water) = 8),
            revision BLOB NOT NULL CHECK(length(revision) = 8),
            runtime_high_water BLOB NOT NULL CHECK(length(runtime_high_water) = 8)
        );
        CREATE TABLE IF NOT EXISTS observations (
            seq BLOB PRIMARY KEY NOT NULL CHECK(length(seq) = 8),
            identity TEXT NOT NULL UNIQUE,
            outcome TEXT NOT NULL,
            dated BLOB, before_size BLOB, after_size BLOB,
            change BLOB, quality BLOB, took BLOB,
            pair_eligible INTEGER NOT NULL CHECK(pair_eligible IN (0,1)),
            {facts},
            {}
        );
        CREATE TABLE IF NOT EXISTS history_attempts (
            owner BLOB NOT NULL REFERENCES observations(seq),
            list TEXT NOT NULL,
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            {attempts},
            PRIMARY KEY(owner,list,ordinal),
            {}
        );
        CREATE TABLE IF NOT EXISTS observation_paths (
            owner BLOB PRIMARY KEY NOT NULL REFERENCES observations(seq),
            source BLOB NOT NULL, output BLOB, file_name TEXT
        );
        CREATE TABLE IF NOT EXISTS operational_log (
            seq BLOB PRIMARY KEY NOT NULL CHECK(length(seq)=8),
            record BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS operational_snapshot (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            record BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS history_pairs ON observations(pair_eligible,seq);
        CREATE INDEX IF NOT EXISTS history_file ON observation_paths(file_name,owner DESC);",
        mapping::shape_checks(false),
        mapping::shape_checks(true)
    ))?;
    for column in [
        "dated",
        "before_size",
        "after_size",
        "change",
        "quality",
        "took",
    ] {
        for direction in ["ASC", "DESC"] {
            database.batch(&format!(
                "CREATE INDEX IF NOT EXISTS history_{column}_{direction}
                 ON observations(({column} IS NULL),{column} {direction},seq DESC)"
            ))?;
        }
    }
    database.execute(
        "INSERT INTO history_meta(singleton,recording_high_water,revision,runtime_high_water)
         VALUES(1,?,?,?) ON CONFLICT(singleton) DO NOTHING",
        &[
            super::database::Value::Blob(number::unsigned(0)),
            super::database::Value::Blob(number::unsigned(0)),
            super::database::Value::Blob(number::unsigned(0)),
        ],
    )?;
    database.batch("PRAGMA user_version=1")?;
    transaction.commit()
}

fn columns(attempt: bool) -> String {
    mapping::columns(attempt)
        .into_iter()
        .map(|(name, kind)| format!("{name} {kind}"))
        .collect::<Vec<_>>()
        .join(",")
}
