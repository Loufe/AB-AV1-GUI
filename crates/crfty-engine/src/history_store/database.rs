use std::path::Path;

use super::{Result, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Candidate {
    Rusqlite,
    Turso,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Value {
    Null,
    Integer(i64),
    Text(String),
    Blob(Vec<u8>),
}

pub(super) enum Database {
    Rusqlite(rusqlite::Connection),
    Turso {
        connection: turso::Connection,
        runtime: tokio::runtime::Runtime,
        _database: turso::Database,
    },
}

impl Database {
    pub(super) fn open(candidate: Candidate, path: &Path) -> Result<Self> {
        let database = match candidate {
            Candidate::Rusqlite => Self::Rusqlite(
                rusqlite::Connection::open(path)
                    .map_err(|error| StoreError::context("open SQLite History store", error))?,
            ),
            Candidate::Turso => {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .map_err(|error| StoreError::context("create Turso runtime", error))?;
                let path = path
                    .to_str()
                    .ok_or_else(|| StoreError("Turso database path is not UTF-8".to_owned()))?;
                let database = runtime
                    .block_on(turso::Builder::new_local(path).build())
                    .map_err(|error| StoreError::context("open Turso History store", error))?;
                let connection = database
                    .connect()
                    .map_err(|error| StoreError::context("connect Turso History store", error))?;
                Self::Turso {
                    connection,
                    runtime,
                    _database: database,
                }
            }
        };
        database.query("PRAGMA journal_mode=WAL", &[])?;
        database.query("PRAGMA synchronous=FULL", &[])?;
        database.query("PRAGMA foreign_keys=ON", &[])?;
        for (pragma, expected) in [
            ("synchronous", Value::Integer(2)),
            ("foreign_keys", Value::Integer(1)),
            ("journal_mode", Value::Text("wal".to_owned())),
        ] {
            let rows = database.query(&format!("PRAGMA {pragma}"), &[])?;
            if rows.first().and_then(|row| row.first()) != Some(&expected) {
                return Err(StoreError(format!(
                    "History durability setting {pragma} was not applied"
                )));
            }
        }
        Ok(database)
    }

    pub(super) fn batch(&self, sql: &str) -> Result<()> {
        match self {
            Self::Rusqlite(connection) => connection
                .execute_batch(sql)
                .map_err(|error| StoreError::context("execute SQLite batch", error)),
            Self::Turso {
                connection,
                runtime,
                ..
            } => runtime
                .block_on(connection.execute_batch(sql))
                .map_err(|error| StoreError::context("execute Turso batch", error)),
        }
    }

    pub(super) fn execute(&self, sql: &str, parameters: &[Value]) -> Result<u64> {
        match self {
            Self::Rusqlite(connection) => {
                let parameters = parameters.iter().map(sqlite_value);
                let changed = connection
                    .execute(sql, rusqlite::params_from_iter(parameters))
                    .map_err(|error| StoreError::context("execute SQLite statement", error))?;
                u64::try_from(changed)
                    .map_err(|error| StoreError::context("SQLite change count", error))
            }
            Self::Turso {
                connection,
                runtime,
                ..
            } => {
                let parameters: Vec<_> = parameters.iter().map(turso_value).collect();
                runtime
                    .block_on(connection.execute(sql, parameters))
                    .map_err(|error| StoreError::context("execute Turso statement", error))
            }
        }
    }

    pub(super) fn query(&self, sql: &str, parameters: &[Value]) -> Result<Vec<Vec<Value>>> {
        match self {
            Self::Rusqlite(connection) => {
                let mut statement = connection
                    .prepare(sql)
                    .map_err(|error| StoreError::context("prepare SQLite query", error))?;
                let columns = statement.column_count();
                let mut rows = statement
                    .query(rusqlite::params_from_iter(
                        parameters.iter().map(sqlite_value),
                    ))
                    .map_err(|error| StoreError::context("query SQLite store", error))?;
                let mut result = Vec::new();
                while let Some(row) = rows
                    .next()
                    .map_err(|error| StoreError::context("read SQLite row", error))?
                {
                    let mut values = Vec::new();
                    for column in 0..columns {
                        let value: rusqlite::types::Value = row
                            .get(column)
                            .map_err(|error| StoreError::context("read SQLite value", error))?;
                        values.push(match value {
                            rusqlite::types::Value::Null => Value::Null,
                            rusqlite::types::Value::Integer(value) => Value::Integer(value),
                            rusqlite::types::Value::Text(value) => Value::Text(value),
                            rusqlite::types::Value::Blob(value) => Value::Blob(value),
                            rusqlite::types::Value::Real(_) => {
                                return Err(StoreError(
                                    "unexpected floating-point History fact".to_owned(),
                                ));
                            }
                        });
                    }
                    result.push(values);
                }
                Ok(result)
            }
            Self::Turso {
                connection,
                runtime,
                ..
            } => runtime.block_on(async {
                let mut rows = connection
                    .query(sql, parameters.iter().map(turso_value).collect::<Vec<_>>())
                    .await
                    .map_err(|error| StoreError::context("query Turso store", error))?;
                let mut result = Vec::new();
                while let Some(row) = rows
                    .next()
                    .await
                    .map_err(|error| StoreError::context("read Turso row", error))?
                {
                    let mut values = Vec::new();
                    for column in 0..row.column_count() {
                        let value = row
                            .get_value(column)
                            .map_err(|error| StoreError::context("read Turso value", error))?;
                        values.push(match value {
                            turso::Value::Null => Value::Null,
                            turso::Value::Integer(value) => Value::Integer(value),
                            turso::Value::Text(value) => Value::Text(value),
                            turso::Value::Blob(value) => Value::Blob(value),
                            turso::Value::Real(_) => {
                                return Err(StoreError(
                                    "unexpected floating-point History fact".to_owned(),
                                ));
                            }
                        });
                    }
                    result.push(values);
                }
                Ok(result)
            }),
        }
    }

    pub(super) fn transaction(&mut self, write: bool) -> Result<Transaction<'_>> {
        self.batch(if write { "BEGIN IMMEDIATE" } else { "BEGIN" })?;
        Ok(Transaction {
            database: self,
            committed: false,
        })
    }
}

pub(super) struct Transaction<'a> {
    pub(super) database: &'a Database,
    committed: bool,
}

impl Transaction<'_> {
    pub(super) fn commit(mut self) -> Result<()> {
        self.database.batch("COMMIT")?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.committed
            && let Err(error) = self.database.batch("ROLLBACK")
        {
            tracing::error!("rollback History transaction failed: {error}");
        }
    }
}

fn sqlite_value(value: &Value) -> rusqlite::types::Value {
    match value {
        Value::Null => rusqlite::types::Value::Null,
        Value::Integer(value) => rusqlite::types::Value::Integer(*value),
        Value::Text(value) => rusqlite::types::Value::Text(value.clone()),
        Value::Blob(value) => rusqlite::types::Value::Blob(value.clone()),
    }
}

fn turso_value(value: &Value) -> turso::Value {
    match value {
        Value::Null => turso::Value::Null,
        Value::Integer(value) => turso::Value::Integer(*value),
        Value::Text(value) => turso::Value::Text(value.clone()),
        Value::Blob(value) => turso::Value::Blob(value.clone()),
    }
}
