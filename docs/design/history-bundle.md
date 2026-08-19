# History bundle

Status: working design note; recorded verdicts are authoritative and survive, everything else is open

## Purpose and boundary

One canonical, versioned, portable History bundle serves personal transfer, deliberate sharing, import, and later pooled research. This document records the decided direction and the open packaging, scope, and disclosure questions. The full statistical schema, manifest, validation, and version evolution are specified when the bundle is built; the settled result becomes `docs/HISTORY_BUNDLE.md`, replacing `docs/HISTORY_IMPORT.md`.

The import exchange format is not the bundle. Import v1 requires a readable path in every record, so a scrubbed store structurally cannot emit it, and it carries no content key, attempt evidence, or tool revisions.

## Recorded verdicts

Labels are stable and cited bare.

**B1, post-3.0 shipping.** The bundle and every export scope ship after the 3.0 release. Import stays in V3. Decisions made for 3.0 must not foreclose the bundle.

**B2, pooled research is the long-term consumer.** Contributed bundles train a shared prior that improves cold-start estimates for installations without local evidence. Contribution is a deliberate user act of packaging and sending a bundle; no automatic telemetry exists in any tier.

**B3, contributor re-keying.** The local content key stays unsalted and deterministic (ADR-019) so records survive reinstalls and re-imports stay idempotent. At the export boundary, every content key is re-keyed with a keyed hash under a per-installation secret. Records within one contributor's bundle still join on identical content, a membership test against candidate media fails without the secret, and rotating the secret unlinks past contributions from future ones.

**B4, coarsened research scope.** Exact sizes, durations, resolutions, and timings are ordinary one at a time and distinctive in combination (`docs/HISTORY.md`), so the research scope coarsens them into buckets that keep their predictive value while breaking the fingerprint. The pathless personal scope remains pseudonymous, never anonymous.

## Open decisions

- Directory, archive, stream, or other inspectable packaging.
- Default readable-source behaviour and its confirmation flow.
- Which export scopes exist: complete, selected, filtered, date-range.
- Validation, cancellation, partial artifacts, and unsupported feature payloads.
- Released bundle-version acceptance and translation policy.
- The submission channel and its disclosure: what a contributor is shown before sending, what the training set retains, and how a shared prior is distributed back.
- The coarsening grid for the research scope, fixed against a measured utility loss rather than chosen without evidence.

## Distillation

Settled rules move to `docs/HISTORY_BUNDLE.md`, decisions with architectural consequence carry ADRs, and this note is deleted when its subject settles.
