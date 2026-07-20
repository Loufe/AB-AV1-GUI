---
status: accepted
date: 2026-07-20
---

# Take an Engine-Owned Data-Directory Lock

## Context and Problem Statement

The journal has exactly one writer (ADR-002, ADR-004), and that must hold across
processes, not just threads: a second application instance replaying and appending
the same journal would corrupt durable state. The guard previously lived as an OS
lock on the journal file handle itself, which ties exclusivity to that handle's
lifetime — compaction's writer barrier must close and reopen the journal, and on
Windows an append-mode handle cannot always be locked. A GUI-level single-instance
plugin is insufficient because headless or crashed-shell scenarios bypass it while
the engine still owns shared state (#33 §16).

## Decision Drivers

* Exclusivity must survive journal close/reopen (compaction, corruption recovery)
* A second instance must fail with a distinct, user-explainable state
* The lock must vanish with the process, with no stale-state recovery logic
* No new dependencies

## Considered Options

* Keep the OS lock on the journal file handle
* Tauri single-instance plugin in the shell
* Exclusive OS lock on a dedicated lock file in the data directory, acquired by
  the driver before any durable state is read

## Decision Outcome

Chosen option: **a dedicated lock file (`crfty.lock`) in the data directory**,
locked exclusively via `std::fs::File::try_lock` and held for the driver's
lifetime. Acquisition happens before settings load and journal fold (#33 §12), so
a losing instance never reads shared durable state. `TryLockError::WouldBlock`
maps to a typed `AlreadyRunning` error that the shell surfaces as a dedicated
second-instance stream payload rather than a generic degraded reason.

The held OS lock is the signal, never the file's existence: the file is left
behind on exit, and a leftover file from a dead process locks cleanly on the next
start. No unlock, cleanup, or staleness protocol exists.

### Consequences

* Good: The journal handle can close and reopen freely without an exclusivity gap
* Good: Second-instance behavior is a typed state, testable end to end
* Good: Advisory locks release on process death, so crashes need no recovery path
* Bad: The lock is advisory; non-cooperating processes are not blocked (they never
  were — this guards against our own second instance, not tampering)

## More Information

See issue #33, sections 12 and 16, and ADR-004.
