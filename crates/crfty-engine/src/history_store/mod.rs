//! Temporary engine comparison for the shared operational and History store.
//!
//! Candidate adapters share SQL, numeric encodings, and transaction semantics.
//! They remain separate from the application's file-journal driver until the
//! workload proves the replacement (ADR-004).

mod database;
mod mapping;
mod number;
mod operational;
mod query;
mod schema;
mod store;
#[cfg(test)]
mod tests;

pub use database::Candidate;
pub use operational::OperationalState;
pub use query::{Cursor, Direction, Order, OutcomeClass, Page, PageRequest};
pub use store::{Entry, HistoryStore, ImportError, PairPage};

use std::fmt;

#[derive(Debug)]
pub struct StoreError(String);

impl StoreError {
    fn context(context: &str, error: impl fmt::Display) -> Self {
        Self(format!("{context}: {error}"))
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

type Result<T> = std::result::Result<T, StoreError>;
