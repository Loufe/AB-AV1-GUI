---
status: accepted
date: 2026-09-30
---

# Import history as keyed translated observations

## Context and problem statement

ADR-024 makes one immutable observation the unit of History and identifies a translated observation by its import origin and the origin's record key, never by a readable path. The shipped import path predates that decision (ADR-015). It parks each V2 record under its normalized readable path, adopts it onto a content record when a queued file later matches that path, and fills an absent decision instant with the import instant. Adoption is how imported facts acquire a content key and a verdict. The parked inbox, the adopted-path guard set, and retained provenance exist to keep adoption idempotent.

The alpha requires strict, idempotent import v1 into the same observation model as native work, with deterministic duplicate, conflict, and rejection outcomes. The question is how imported history enters History: what identifies a translated observation, what it may claim, whether it still adopts onto current files, and how a re-import, a changed re-export, or an invalid file behaves.

## Decision drivers

* Re-importing the same file must change nothing, so an uncertain commit can be retried safely
* A re-export whose record under a key has changed must surface as a conflict rather than as a second record of the same work
* Identity must survive scrub and must never be a readable path
* A translated observation must claim only what its V2 record holds: no content key, toolchain, operation, or instant that the record never had
* Imported evidence must never gain native standing or reusable analysis, because its toolchain, preset, and decode mode cannot be established after the fact (estimation E4)
* Imported evidence must never suppress required work, because a V2 verdict was reached by an unestablished toolchain about a file V3 has not observed
* Anonymized V2 histories hold real evidence and should not be discarded for lacking a readable path

## Considered options

* Keyed translated observations without adoption, keyed by the V2 record's own primary key, its path hash, under a V2 origin
* Keyed translated observations that still adopt onto content records when a queued file matches their path
* Translated observations keyed by the normalized readable path, as the parked inbox keys records today
* Translated observations keyed by a digest over the record's facts
* Translated observations keyed by the path hash combined with the recorded instant

## Decision outcome

Chosen option: **keyed translated observations without adoption, keyed by the V2 path hash under a V2 origin**. V2 stores exactly one record per path hash, so the key is stable across re-exports of one history, carries no readable path, and exists even for records V2 had already anonymized.

Continued adoption keeps a second route from imported evidence into current state. That route is the parked inbox, guard set, and provenance machinery, and it lends a verdict from an unestablished toolchain. A readable-path key fails the identity driver, because it is a readable path and scrub would delete it. A digest over the facts makes every change a new observation, so a re-export double counts and no conflict is ever detectable. A key that includes the recorded instant churns even on one machine, because V2 rewrote a record's last update on every rescan, so a later re-export turns each rescanned record into a second copy. V2 also wrote that instant as naive local time, which the converter interprets in the exporting machine's zone, so a re-export from another zone would shift every key.

Mechanics, fixed by this record:

* A translated observation is identified by the pair of `ImportOrigin::V2History` and a `RecordKey` of exactly 16 lowercase hexadecimal characters, the V2 `path_hash`. Any other key shape does not deserialize, so a path cannot become an identity.
* Import v1 changes in place, keeping `import_version` 1 because no release has consumed it. Each record carries its key as a required field, and the readable path becomes optional. The modification time is dropped, because nothing matches records against files. A `scanned` status is not accepted, because a scan decided nothing and produces no observation.
* The converter changes with the importer. It emits the path hash as the key, drops scanned records, emits anonymized records without a path instead of dropping them, and orders records by key. For a converted record, an AV1 codec or equal recorded source and output sizes when both are known is a conservative signal that a V2 rescan may have replaced source metadata with output metadata. When either holds, the converter omits source size, codec, dimensions, and duration together. These signals can also match an untouched source, and any retained source metadata remains V2-reported rather than verified V3 measurements. A record without a last update exports without an update instant; the converter does not substitute the first-seen instant, which is a different claim.
* A translated observation holds a sparse source (codec, dimensions, duration, size), an outcome of converted, not worthwhile, or analyzed with the quality and sizes the record held, and an optional update instant. The update instant is the V2 record's last update and may postdate the decision, because V2 rewrote it on rescans. The observation has no content key, tool revisions, operation, or start instant. An absent instant stays absent and is never replaced by the import instant or the first-seen instant.
* Import is strict. The file is parsed and every record validated before anything is planned, and one invalid record rejects the whole file with nothing written. A valid file is planned in record order. A new identity inserts, and an identity already present with identical facts is a duplicate. An identity present with different facts is a conflict: the existing observation stands, and the incoming record is reported as changed since its first import and dropped. Records earlier in the same file count as present. Paths never take part in the comparison.
* One import commits as one transaction. A duplicate writes nothing, including a path, so a re-import after a scrub restores no readable path. When path recording is disabled, inserts write no path rows.
* Nothing adopts. Translated observations never enter a content record, a verdict, an Analysis level, or the reusable analysis index, and they decide no standing. Their one eligibility accessor is the imported fact, yielding translated converted, not-worthwhile, and analyzed evidence as a separately labelled cohort. That cohort is never deduplicated against native observations, since the two share no identity. Estimation admits it as the toolchain-unversioned cold-start prior.

Import assumes the one-time move this format exists for: a V2 history that is no longer in use, exported on the machine that produced it. Continuing to use V2 after an import, exporting elsewhere, or merging histories from two installations that share absolute paths all surface as conflicts, and the first import's facts stand.

### Consequences

* Good: Re-import is idempotent by construction, so an uncertain import commit can be retried without harm
* Good: Anonymized V2 records import as pathless evidence instead of being dropped by the converter
* Good: The parked inbox, the adopted-path guard set, retained import provenance, and path-keyed History rows all disappear, along with the path-bearing privacy surfaces they created
* Good: Import no longer fabricates instants
* Bad: A file V2 judged not worthwhile is queueable again, and converting it repeats the full fallback search that V2 already ran
* Bad: Analysis rows no longer show an imported historical level, and a V2-converted file shows no standing until V3 observes it
* Bad: A V2-converted source outside replace mode is protected from reconversion only by the existing-output check
* Bad: A conflict keeps the first import's facts, and no correction path exists for an imported observation short of a store reset
* Bad: Imported and native evidence about the same file cannot be joined, so file-counting aggregates may count that file once in each cohort
* Bad: A record key is an unsalted 64-bit digest of the V2 path and survives scrub, so anyone holding candidate paths can recompute and match it; a scrubbed store with translated observations is pseudonymous, not anonymous

## More information

The observation contract and the logical History model are `docs/HISTORY.md`; the storage workload these rules are tested under is `docs/design/history-storage-workload.md`. ADR-024 fixes the observation unit and ADR-022 the metric tag every imported quality value carries. The accessor and its cohort label are implemented in `crfty_core::Observation::imported_fact`; import planning is `crfty_core::plan_import`.

This record replaces ADR-015 when translated observations are stored. ADR-015 is then deleted, and every document and record that references ADR-015, adoption, or the parked inbox is rewritten to this record, including ADR-009, ADR-016, ADR-017, ADR-018, ADR-019, `docs/ARCHITECTURE.md`, `docs/PLAN.md`, `docs/POLICY.md`, `docs/ANALYSIS.md`, `docs/HISTORY.md`, `docs/HISTORY_IMPORT.md`, `docs/TESTING.md`, and the History design notes. Until then ADR-015 describes the shipped parked path and `docs/HISTORY_IMPORT.md` the shipped file format.
