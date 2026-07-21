---
status: accepted
date: 2026-07-20
---

# Filter Queue Adds at Enqueue

## Context and Problem Statement

The V2 application decided queue membership interactively and late: adds
popped conflict dialogs (replace / keep both), re-adding a path could create
a second copy, one edit wrote a suffix or folder across every pending item,
and Analyze adds were pre-filtered against cached analyses. In V3 every
mutation rule lives in the pure reducer (ADR-002) and the queue replays from
the journal (ADR-004), so queue semantics must be decidable from command
payloads plus durable state — and each deliberate divergence from V2 needs a
recorded rationale. This record covers the queue-command semantics shipped
with issue #41 as one decision.

## Decision Drivers

* Enqueue decisions must be computable in the reducer from command payloads
  (path hash, file stamp) plus durable state — no I/O, no dialogs (ADR-002)
* Replay validation requires a journaled `QueueAdded` to fold to a `Queued`
  item; born-finished rows would weaken that invariant
* Adding a folder where most files are already converted is the routine
  case, not an error — it must not interrupt with modal dialogs
* Facts improve over time: enqueue sees only a path and stamp, while claim
  time holds a content observation, so each tier filters on what it can trust
* The queue shape invariant (finished < active < queued) and active-item
  immutability must survive every command

## Considered Options

* Port V2 semantics: conflict dialogs with keep-both, duplicate items per
  path, bulk writes across pending items, Analyze adds pre-filtered
* Filter at add with a typed summary; one item per path; per-item edit;
  retry in place; Analyze adds unfiltered
* Enqueue everything and skip only at claim time, as visible skipped rows

## Decision Outcome

Chosen option: **filter at add with a typed summary**, together with the
command semantics below.

`AddMany` runs each request through the pure enqueue policy: files whose
decided verdict still describes the content on disk (converted output
recognized by stamp, not-worthwhile, already AV1/Matroska) never become queue
items. All dispositions of one batch accumulate into a single
`QueueAddSummary` ephemeral with counts by typed reason, rendered as one
neutral toast — never a dialog. Enqueueing everything as visible skipped rows
was rejected because it would either journal born-finished items (weakening
the `QueueAdded`-folds-to-`Queued` replay invariant) or flood the queue with
rows that carry no action. Claim-time short-circuits are the complement, not
a substitute: an item already queued whose content key resolves to a decided
verdict at claim finishes as a visible skipped row, because by then the row
exists and the observation is authoritative. `AnalysisIntent::Refresh` is the
explicit escape hatch past verdict-based filtering.

One item per path: re-adding a path that is already queued is counted in the
summary, not duplicated and not offered a keep-both dialog — changing what
should happen to a queued path is what `Edit` is for. Editing is strictly
per-item (operation, intent, output target, overwrite) and only while no
session is running — a run's work is frozen at start, so operation and
output are not editable mid-run; V2's write-to-all-items bulk edits are
deliberately not
ported, since a frontend loop over per-item commands batches into one fsync
anyway. `Retry` resets a finished item in place and moves it to the end of
the queue, preserving the shape invariant instead of re-adding. Analyze adds
are not pre-filtered against cached analyses: claim-time reuse makes a
re-analysis of a cached file near-instant, and filtering earlier would
duplicate profile matching on weaker facts.

### Consequences

* Good: Ineligible files never occupy the queue or the journal; routine adds
  stay non-interactive end to end
* Good: Every filtering rule is pure, deterministic, and covered by reducer
  decision tables; replay invariants stay strict
* Bad: A skipped file is visible only as a count in the summary toast, not
  as an inspectable row — finding out *why* a specific file was skipped
  requires re-adding with `Refresh` or consulting history
* Bad: Bulk operations cost one command per item on the wire (mitigated by
  driver batching)

## More Information

Implemented across issue #41; scope and design verdicts in issue #33 §11.
See ADR-002 (reducer owns mutation), ADR-004 (journal replay), the enqueue
policy in `crates/crfty-core/src/policy.rs`, and the replay validation in
`crates/crfty-core/src/journal.rs`.
