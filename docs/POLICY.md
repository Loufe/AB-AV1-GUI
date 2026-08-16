# Conversion Policy

Policy is the single authority on what CRFty will do with a file: whether it is eligible, whether an earlier judgment already answers for it, which stored analysis may be reused, and which of Analyze, Encode, Remux, or Skip a claimed job becomes. It is pure domain logic in `crates/crfty-core/src/policy.rs`, taking facts as arguments and returning a decision. No scanner, worker, shell command, or view repeats any of these checks.

## Where policy runs

Policy is evaluated at two moments, from different facts.

- Enqueue, `evaluate_enqueue`: decides before a queue item exists. A decision here is a skip reason counted into the add summary; it never creates an item, and enqueue-time reasons are never terminal outcomes. Facts are the cached path binding plus a cheap stat-derived identity, never a probe. See ADR-013 (filter queue adds at enqueue).
- Claim, `select_job_action`: decides what the claimed job actually does. Facts are a fresh preflight observation of the file (or the content record when the probe fails) and the execution settings frozen into the `JobSpec` at that moment.

The tiers differ deliberately. Execution targets are injected at claim rather than at add, so only the claim tier can compare a request's fallback floor against a recorded one. Content identity is authoritative at claim, because the record was resolved by the observation's `ContentKey`, whereas the enqueue tier reasons about a path and must first establish that its cached facts still describe the file there.

## Media eligibility

One function, `evaluate_eligibility`, answers both tiers and yields Process, Remux, or Skip.

- Inputs below 921,600 pixels after rotation are skipped as `LowResolution { pixels, minimum }`. Post-rotation dimensions swap width and height when the rotation is 90 modulo 180; the probe normalizes rotation into 0 to 359 degrees first. This is the 720p floor the Python UI intended but never connected.
- AV1 already in Matroska is skipped as `AlreadyAv1Matroska`.
- AV1 in any other container becomes a Remux when the operation is Convert. Under Analyze it is not remuxed: the file goes to a normal search.
- Everything else is Process.

The container fact is taken from the file extension: a case-insensitive `.mkv` is Matroska, and anything else carries the probed format name as `Other`. A misnamed file is therefore classified by its name, not by its real container.

At claim, a media-fact skip outranks everything, including verdict reuse and the remux branch. At enqueue, the same eligibility check runs last, and only when the cached metadata has been shown fresh, so a file whose facts are unknown or stale is accepted rather than pre-judged.

There are no audio controls; ab-av1's defaults apply. Audio stream metadata is probed for History display only and takes no part in eligibility or remux decisions.

## Remux

A remux is a container change with no video re-encode: FFmpeg runs `-map 0 -map_metadata 0 -map_chapters 0 -c copy -f matroska` into the staging file, so every stream, all metadata, and all chapters are copied.

No stream is dropped, re-encoded, or otherwise adjusted to make the copy fit Matroska. If FFmpeg refuses the copy, the job fails with a remux-run failure carrying a scrubbed stderr tail. A failed remux never falls through to a lossy encode.

A remux settles through the same output transaction as an encode and produces a `Remuxed` verdict naming the settled output's content key.

## Verdicts and freshness

A content record carries at most one standing verdict, the judgment of the latest decisive run: `Converted`, `Remuxed`, or `NotWorthwhile`. Analysis results are facts rather than verdicts, and Stopped, Skipped, and Failed decide nothing.

`verdict_applies` answers whether a decided verdict still describes the file at a candidate path, without a probe.

- `Converted` and `Remuxed` apply only while the file on disk is the produced output: timestamps must be engine-confirmed reliable, and the live destructive identity (filesystem file id, size, and a known modification time) must equal the settled output identity of the verdict's source run. A missing file, a changed file, or a transaction that never settled means the verdict no longer answers for that path.
- `NotWorthwhile` is a judgment about input content, so it applies whenever the record was resolved by content identity. Whether its targets satisfy a new request is a matter of target policy, not freshness.
- An adopted verdict (imported, with no source run) has no transaction to resolve. Its `Converted` form therefore never applies at a path, because the imported output was never content-hashed and cannot be proven to be the file on disk. Its `NotWorthwhile` form applies by content identity as usual.

Enqueue evaluates in this order.

1. The same input path is already queued and unfinished: `AlreadyQueued`, an add-summary reason that is never a terminal outcome.
2. No cached binding or no record for that content: accepted.
3. A settled replace-mode output sitting at the input path is recognized first, by identity against the settled output rather than by the binding, which is stale by construction there: `AlreadyConverted`.
4. Anything else requires a fresh binding, meaning reliable timestamps and a live identity equal to the one the content key was recorded under. Without it, the add is accepted.
5. With a fresh binding, a Convert add whose record carries a decisive verdict is skipped: `ProbableDuplicate` for `Converted` or `Remuxed`, `NotWorthwhile` for `NotWorthwhile`. The enqueue tier cannot compare fallback floors, so it defers to the standing judgment.
6. Media eligibility, as above.

Claim evaluates in this order: media-fact skip, then verdict short-circuit, then remux, then the analyze or encode action. The verdict short-circuit applies to Convert only and skips `Converted` or `Remuxed` content as `ProbableDuplicate` wherever the bytes now live, catching cross-path copies and verdicts that became decisive after the add. A `NotWorthwhile` verdict skips only when the request's fallback floor is at or above the recorded floor: failure at a floor implies failure at every higher target, while a lower floor is untried ground and proceeds.

`AnalysisIntent::Refresh` bypasses every verdict-based skip, at both tiers. It is the explicit "do it again" escape hatch and cannot make an ineligible file eligible: media-fact skips still apply. Analyze adds are never filtered on cached analyses, because claim-time reuse makes a redundant Analyze near-instant and enqueue-time profile matching would duplicate selection on weaker facts.

Content identity is the sampled probabilistic `ContentKey`, so duplicate reuse and skipping are probabilistic by design. That authority extends to reusing a record, reusing an analysis, and skipping a probable duplicate with a visible reason. It never authorizes deleting, overwriting, or cleaning up a file.

## Analysis reuse and VMAF targets

Each content record stores analyses as a map from `AnalysisProfile` to a map from the successful `VmafTarget` to the result. The profile is the reuse identity: preset, maximum encoded percent, sample count, sample duration, thorough flag, decode mode, and the ab-av1, FFmpeg, and encoder revisions. Reuse requires exact profile equality, so a tool upgrade or a settings change simply re-searches.

Selection for a requested target T works as follows.

- A stored result whose successful target is at or above T qualifies. The lowest such target is chosen, which prefers an exact T when one exists and otherwise avoids needlessly oversized output.
- A result whose successful target is below T qualifies only when it came from an identical ladder: the same requested target, fallback floor, and fallback step. That result is what re-running the request would produce, so reusing it is not a quality regression. Otherwise a lower result never satisfies a higher request.
- Every candidate is revalidated against the claim before it is used, and a selected result that fails validation rejects the claim rather than silently downgrading it.

The requested target, floor, and step come from execution settings, whose production defaults are a target of 95, a floor of 90, and a step of 1. The fallback ladder lives entirely in the search phase: on each failure the target steps down until the next step would fall below the floor. Each failed target is recorded as an attempt with its last measurement, and an exhausted ladder is a `NotWorthwhile` outcome carrying the whole cascade, so a later retry can tell "95 failed but 93 succeeded" from "everything down to the floor failed".

## Hardware decode

The `hardware_decode` setting (on by default) resolves at claim into a decode preference. Software-only pins the profile's decode mode to software. Hardware-preferred asks the engine to pick the first codec-appropriate decoder that the discovered FFmpeg actually offers, falling back to software when none is available; availability is probed once per decoder and cached for the session.

The decode mode that a search actually ran with is part of the analysis identity and is decoder-granular, per ADR-007 (pin the actual decode mode in the analysis identity). An analysis recorded under one hardware decoder is not returned for a job that would run under another, or under software.

Hardware failure retries once with software, in both phases, without rewriting the requested `JobSpec`.

- Search: any failure other than "no good CRF" under a hardware profile restarts the whole VMAF ladder with the software profile, discarding the attempts measured under hardware so the recorded result is honest about the profile it ran with.
- Encode: a failed hardware encode recreates the staging file inside the still-unsettled output transaction and retries once with software. Once a transaction is abandoned the ledger refuses to restage, which makes retry-after-abandonment unrepresentable.

`permitted_profiles` is the gate that admits the divergence: a run may durably record under its prepared profile, plus that profile's software variant when the prepared profile decodes in hardware. A software-prepared run has no wider ladder. Which decode mode ran is preserved in the run's terminal evidence.

## Probe failure

Probe failure is fail-open at both tiers, so a file is never rejected for facts CRFty could not obtain.

- At claim, a failed preflight probe is logged and the job continues with no observation. Metadata falls back to the content record if one exists; with no metadata at all, eligibility is not evaluated and the job proceeds to search or encode. ab-av1 becomes the arbiter of whether the file can be converted. Decode mode falls back to software, because no codec is known to select a decoder for.
- At enqueue, an add with no path hash, no binding, or no record is accepted.

## Output destination and overwrite

Every output is Matroska. The item's output target resolves at claim into a final path and a replacement mode.

- Replace: an input already named `.mkv` (case-insensitive) is rewritten at its own path, keeping the original entry. Any other input produces a sibling `.mkv`, and the original is retired, meaning it is deleted only after the new output is committed.
- Suffix: stem plus suffix plus `.mkv`, beside the input, keeping the original. The suffix is a filename fragment, not a path: separators, control characters, Windows-invalid characters, and a trailing dot or space are refused, as are names that would collide with reserved Windows device names.
- Separate folder: the input's parent, taken relative to the configured source root, is mirrored under the target directory. An input outside that root is refused, and a relative path that is not plainly contained is refused.

Overwrite is decided per item, either following the current output settings or pinned to allow or deny, resolved at claim. An existing destination that is not the input path itself, without permission to overwrite, ends the job as `Skipped` with reason `OutputExists`. Editing the item to allow overwriting and retrying is how that skip is resolved, without touching an active job.

## Hardlinks

Promotion renames the staging file over the final path, so a same-path replacement replaces only that directory entry. A hardlinked sibling keeps the original content under its own entry and is never rewritten in place. The engine's durability contract tests assert this directly.

## Two-phase CONVERT

CONVERT is a typed CRF search, then a durable analysis checkpoint, then an encode at the selected CRF. ANALYZE stops at the checkpoint. Both share one search runner.

- The checkpoint is a record-analysis command committed before the encode begins, so an interruption never discards a completed search.
- When a stored analysis qualifies for the claim, the search phase is skipped entirely and the encode uses the stored CRF.
- The VMAF fallback loop exists only in the search phase. An encode with a known CRF never falls back on quality; its only retry is the hardware-to-software decode retry above.
- ANALYZE creates no output transaction and produces no file. It ends as `Analyzed`, which is a fact rather than a verdict.
- A CONVERT job opens its output transaction only after the analysis is settled, and the ledger refuses to start one for an encode with no recorded analysis.
