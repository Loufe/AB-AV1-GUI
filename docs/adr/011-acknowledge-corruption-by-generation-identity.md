---
status: accepted
date: 2026-07-20
---

# Acknowledge Corruption by Generation Identity

## Context and Problem Statement

A journal that fails replay validation degrades the driver: reads keep working
over the valid prefix, mutation is rejected, and the file is preserved
byte-identical as evidence (ADR-004, ADR-009). Recovery discards the
unreadable suffix — an irreversible, operator-consented data discard. The
acknowledgement protocol must guarantee the operator discards exactly the
bytes they were shown, and the rewrite must never widen the loss beyond that
suffix, even across crashes and failed rebuilds.

## Decision Drivers

* Consent must bind to specific bytes: an acknowledgement issued against one
  corruption must never discard a different, later one
* No crash window may lose the valid prefix or leave no journal at all
* The corrupt generation must survive recovery for post-mortem inspection
* A failed or interrupted recovery must be retryable, not fatal
* Degraded state lives outside `AppState` (ADR-002), so the reducer cannot own
  the acknowledgement

## Considered Options

* A global "degraded acknowledged" boolean command
* Acknowledge by signature of the unreadable suffix, computed at detection
* Archive by renaming the corrupt journal before rebuilding
* Archive by copying, then atomically replace the journal with a compacted
  snapshot of the valid prefix
* A sidecar ack-file consumed at next startup

## Decision Outcome

Chosen option: **acknowledge by suffix signature, archive by copy, rebuild by
forced compaction**.

Core `replay` stamps every corruption report with a signature of the whole
unreadable suffix — its byte length and a BLAKE2b digest, computed at
detection over `bytes[valid_prefix_len..]`. Per-record identity is impossible
by definition: the suffix is precisely the part that cannot be parsed. The
`AcknowledgeCorruption` command carries a signature back, and the driver — not
the reducer, since degraded state deliberately lives outside `AppState` —
intercepts it and accepts only an exact match against the standing report. A
boolean acknowledgement was rejected because it consents to a state, not to
bytes: raced against a newer corruption it would silently discard a suffix
the operator never saw.

On a match the driver copies the journal to a timestamped `.corrupt-` sibling
(fsynced, like the config store's quarantine), then reuses forced compaction
(ADR-009) to atomically replace the journal with one snapshot of the
valid-prefix state, sequence numbering continuing. The archive is a copy,
never a rename: compaction's failure path reopens the journal path with
create semantics, so a rename followed by a failed rebuild would materialize
an empty journal — losing the valid prefix. With copy-then-replace, every
crash or failure point leaves the corrupt original in place; the next start
merely degrades again and the acknowledgement is retried. Success clears the
degraded state and announces `Recovered` on the stream, so recovery needs no
restart. A sidecar ack-file was rejected as a second artifact with its own
crash protocol, deferring recovery to a restart for no gain.

### Consequences

* Good: A stale or fabricated acknowledgement is structurally inert — wrong
  signature, no effect, file untouched
* Good: No crash window loses more than the operator consented to; the
  archive preserves the full corrupt generation for inspection
* Good: Rebuild reuses the compaction path's crash-safety and its tests
* Bad: The UI must echo the signature it observed rather than sending a bare
  confirmation
* Bad: Archives accumulate until manually deleted — acceptable for an event
  that should be rare and evidence-worthy

## More Information

See issue #33 section 10, issue #39 phase 3, ADR-002, ADR-004, and ADR-009.
