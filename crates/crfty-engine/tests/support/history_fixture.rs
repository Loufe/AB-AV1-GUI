use std::{
    error::Error,
    io::{self, BufRead, Write},
    path::Path,
    process,
};

use crfty_core::{ImportCandidate, SessionState, UnixMillis};
use crfty_engine::history_store::{Candidate, HistoryStore};

mod history_facts;

fn boundary(cut: &str) {
    if cut == "before" {
        abort_at("before");
    }
}

fn abort_at(label: &str) -> ! {
    println!("{label}");
    if let Err(error) = io::stdout().flush() {
        eprintln!("flush crash boundary: {error}");
        process::exit(1);
    }
    process::abort()
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let candidate = match arguments.next().as_deref() {
        Some("rusqlite") => Candidate::Rusqlite,
        Some("turso") => Candidate::Turso,
        _ => return Err("unknown storage candidate".into()),
    };
    let directory = arguments.next().ok_or("missing synthetic data directory")?;
    let action = arguments.next().ok_or("missing crash action")?;
    let cut = arguments.next().ok_or("missing crash boundary")?;
    let mut store = HistoryStore::open(candidate, Path::new(&directory))?;
    match action.as_str() {
        "lock" => {
            println!("locked");
            io::stdout().flush()?;
            let mut line = String::new();
            io::stdin().lock().read_line(&mut line)?;
            abort_at("released");
        }
        "terminal" => {
            store.append_operations(&history_facts::prepared(), true)?;
            store.append_operations_with_commit_hook(&[history_facts::stopped()], true, || {
                boundary(&cut)
            })?;
        }
        "import" => {
            let observation: crfty_core::TranslatedObservation = serde_json::from_str(
                include_str!(
                    "../../../crfty-core/tests/fixtures/observations/translated/valid/converted_without_sizes.json"
                ),
            )?;
            let mut second = observation.clone();
            second.record_key = crfty_core::RecordKey::new("000000000000abcd".to_owned())?;
            store
                .import_with_commit_hook(
                    vec![
                        ImportCandidate {
                            observation,
                            path: Some("synthetic-import.mkv".into()),
                        },
                        ImportCandidate {
                            observation: second,
                            path: Some("synthetic-import.mkv".into()),
                        },
                    ],
                    true,
                    || boundary(&cut),
                )
                .map_err(|error| format!("synthetic import failed: {error:?}"))?;
        }
        "compact" => {
            store.append_operations(&history_facts::prepared(), true)?;
            store.append_operations(&[history_facts::stopped()], true)?;
            store.compact_operations_with_commit_hook(
                SessionState::Idle,
                UnixMillis(200),
                || boundary(&cut),
            )?;
        }
        _ => return Err("unknown crash action".into()),
    }
    abort_at("after")
}

fn main() {
    if let Err(error) = run() {
        eprintln!("History crash fixture failed: {error}");
        process::exit(1);
    }
}
