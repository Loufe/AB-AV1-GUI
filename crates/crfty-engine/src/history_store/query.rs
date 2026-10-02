use std::collections::BTreeSet;

pub use crfty_core::HistoryOutcome as OutcomeClass;
use crfty_core::ObservationId;
use serde::Deserialize;

use super::{Result, StoreError, database::Value, number};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Order {
    Date,
    File,
    Before,
    After,
    Change,
    Quality,
    Took,
    Recorded,
}

impl Order {
    pub(super) fn column(self) -> &'static str {
        match self {
            Self::Date => "o.dated",
            Self::File => "p.file_name",
            Self::Before => "o.before_size",
            Self::After => "o.after_size",
            Self::Change => "o.change",
            Self::Quality => "o.quality",
            Self::Took => "o.took",
            Self::Recorded => "o.seq",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Direction {
    Ascending,
    Descending,
}

pub(super) fn outcome_text(outcome: OutcomeClass) -> &'static str {
    match outcome {
        OutcomeClass::Converted => "Converted",
        OutcomeClass::Remuxed => "Remuxed",
        OutcomeClass::NotWorthwhile => "NotWorthwhile",
        OutcomeClass::Analyzed => "Analyzed",
        OutcomeClass::Failed => "Failed",
        OutcomeClass::Stopped => "Stopped",
        OutcomeClass::Incomplete => "Incomplete",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueryIdentity {
    order: Order,
    direction: Direction,
    outcomes: BTreeSet<OutcomeClass>,
    search: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Cursor {
    query: QueryIdentity,
    value: Value,
    seq: u64,
}

pub struct PageRequest<'a> {
    pub order: Order,
    pub direction: Direction,
    pub outcomes: Option<BTreeSet<OutcomeClass>>,
    pub search: Option<String>,
    pub after: Option<&'a Cursor>,
    pub limit: usize,
}

#[derive(Debug)]
pub struct Page {
    pub ids: Vec<ObservationId>,
    pub matched: u64,
    pub more: bool,
    pub revision: u64,
    pub next: Option<Cursor>,
}

pub(super) struct SqlQuery {
    identity: QueryIdentity,
    pub(super) filter: String,
    pub(super) parameters: Vec<Value>,
    pub(super) following: String,
    pub(super) following_parameters: Vec<Value>,
    pub(super) ordering: String,
    pub(super) column: &'static str,
}

impl SqlQuery {
    pub(super) fn build(request: &PageRequest<'_>) -> Result<Self> {
        if !(1..=200).contains(&request.limit) {
            return Err(StoreError(
                "History page limit must be in 1..=200".to_owned(),
            ));
        }
        let identity = QueryIdentity {
            order: request.order,
            direction: request.direction,
            outcomes: request.outcomes.clone().unwrap_or_else(|| {
                [
                    OutcomeClass::Converted,
                    OutcomeClass::Remuxed,
                    OutcomeClass::NotWorthwhile,
                    OutcomeClass::Analyzed,
                ]
                .into_iter()
                .collect()
            }),
            search: request
                .search
                .as_deref()
                .filter(|search| !search.is_empty())
                .map(str::to_owned),
        };
        if request.after.is_some_and(|cursor| cursor.query != identity) {
            return Err(StoreError(
                "History cursor belongs to a different query".to_owned(),
            ));
        }
        let mut parameters = Vec::new();
        let filter = if identity.outcomes.is_empty() {
            "0".to_owned()
        } else {
            let placeholders = vec!["?"; identity.outcomes.len()].join(",");
            parameters.extend(
                identity
                    .outcomes
                    .iter()
                    .map(|outcome| Value::Text(outcome_text(*outcome).to_owned())),
            );
            format!("o.outcome IN ({placeholders})")
        };
        let filter = if let Some(search) = &identity.search {
            parameters.push(Value::Text(search.to_ascii_lowercase()));
            format!("({filter}) AND instr(p.file_name,?) > 0")
        } else {
            filter
        };
        let column = request.order.column();
        let (direction, comparison) = match request.direction {
            Direction::Ascending => ("ASC", ">"),
            Direction::Descending => ("DESC", "<"),
        };
        let mut following_parameters = parameters.clone();
        let following = match request.after {
            None => "1".to_owned(),
            Some(cursor) if cursor.value == Value::Null => {
                following_parameters.push(Value::Blob(number::unsigned(cursor.seq)));
                format!("{column} IS NULL AND o.seq < ?")
            }
            Some(cursor) => {
                following_parameters.extend([
                    cursor.value.clone(),
                    cursor.value.clone(),
                    Value::Blob(number::unsigned(cursor.seq)),
                ]);
                format!(
                    "{column} IS NULL OR {column} {comparison} ? OR ({column} = ? AND o.seq < ?)"
                )
            }
        };
        Ok(Self {
            identity,
            filter,
            parameters,
            following,
            following_parameters,
            ordering: format!("({column} IS NULL) ASC,{column} {direction},o.seq DESC"),
            column,
        })
    }

    pub(super) fn cursor(&self, value: Value, seq: u64) -> Cursor {
        Cursor {
            query: self.identity.clone(),
            value,
            seq,
        }
    }
}
