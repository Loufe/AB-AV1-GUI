---
status: accepted
date: 2026-07-20
---

# Scrub paths inside the log sink

## Context and problem statement

Log lines routinely embed file paths, and paths identify people. V2 anonymized them with a `logging.Filter` on each handler; V3 uses `tracing`, whose events fan out through layers that format independently. The scrubber must see every line destined for disk with no unfiltered window, including lines emitted before settings are loaded, which is a startup-order conflict: tracing comes up first (the startup order is in `docs/design/lifecycle.md`), but whether to anonymize is itself a setting. Settings changes must also retarget the scrubber and the log directory at runtime without restarting the subscriber, and hashes must stay byte-identical to V2's (BLAKE2b-128 truncated to 12 hex chars) so hashes in old and new logs refer to the same files.

## Decision drivers

* No log line may reach disk unscrubbed while anonymization is on
* Anonymization state must be readable before the settings subsystem exists
* Runtime reconfiguration (toggle, folder placeholders, log directory) without tearing down the tracing subscriber
* Hash parity with V2 so `tools/hash_lookup.py`-style reverse lookup spans eras
* Workspace lint policy: no `unwrap`/`expect`/indexing in production code

## Considered options

* `tracing-subscriber` reload handles that swap filter/writer layers on change
* A scrubbing `Layer` that rewrites event fields before the fmt layers
* One process-global sink (`OnceLock<Arc<LogControl>>`) whose `MakeWriter` scrubs complete lines inside the write path, mutated in place on reconfigure

## Decision outcome

Chosen option: **one process-global sink that scrubs inside the write path**. The subscriber's layers are installed once and never change. Everything a setting can alter (scrub toggle, configured-folder placeholders, target directory, open file) lives behind one sink mutex. Formatted lines are buffered per writer, split on newlines, and passed through the scrubber before any byte reaches the file. `init` reads the config without quarantining it, leaving the driver as the sole owner of settings loading, so the initial lines honour the persisted toggle. `reconfigure` runs only after a settings write durably succeeds; a rejected write must not change what the logs anonymize or where they land. Directory changes open the new file before swapping, so a failure keeps the existing sink. Sink-internal failures report through `eprintln!` once, never through tracing, because emitting inside the write path would deadlock on the sink mutex.

Retroactive scrubbing (`scrub_log_files`) reuses the same scrubber under the same lock: it closes the active file, rewrites each log atomically (temp + sync + rename), and reopens in append mode. An idempotency guard recognizes already-anonymized `file_<hash>` tokens so repeated scrubs are no-ops, a deliberate fix over V2, whose scrub re-hashed its own output.

### Consequences

* Good: The no-unfiltered-window guarantee is structural: scrubbing sits below every layer, so no future layer can bypass it
* Good: No reload-handle plumbing; reconfiguration is one mutex-guarded update
* Good: V2 and V3 hashes are interchangeable, verified by frozen parity vectors
* Bad: Every logged line pays a regex pass while anonymization is on
* Bad: A global `OnceLock` sink is process-wide state; tests exercise the scrubber and rotation logic as pure functions rather than through `init`

## More information

See `docs/design/lifecycle.md` (startup order) and ADR-002 (the driver as sole settings owner). Detection patterns and hash parity vectors live in `crfty-engine/src/logging/privacy.rs`.
