---
status: accepted
date: 2026-10-11
---

# Invoke the official ab-av1 CLI

## Context and problem statement

CRFty needs structured quality-search results and reliable cancellation in a long-lived desktop application. The official ab-av1 CLI provides NDJSON search results. A fresh CLI process isolates its internal tasks and global state from CRFty, so the application does not need a patched library lifecycle API.

## Decision drivers

* Use the official upstream release without a fork or source patch.
* Parse machine protocols rather than terminal text.
* Keep quality search and final encoding settings aligned.
* Own process containment, pipe drainage, temporary files, and output settlement.

## Considered options

* Embed a patched library with operation ownership added upstream.
* Run a helper linked to a patched library.
* Invoke the official CLI for quality search and FFmpeg directly for encoding.

## Decision outcome

Chosen option: **invoke the official CLI for quality search and FFmpeg directly for encoding**. Require the JSON search contract provided by ab-av1 0.11.7, verify it with a synthetic search at session start, and identify the selected executable bytes in analysis provenance. Tools are user-supplied under ADR-023; `CRFTY_AB_AV1`, the Settings path, and PATH are the discovery tiers.

Search consumes bounded NDJSON lines and requires an exit status consistent with its terminal message. Completed trial telemetry does not provide per-frame search progress, so the search phase is indeterminate. Final encoding uses FFmpeg's `-progress pipe:1` protocol. Both commands explicitly use libsvtav1, the selected preset, 10-bit pixels, a 240-frame keyframe interval, scene detection disabled, and the selected decoder. Fractional CRF reaches SVT through `-svtav1-params`.

CLI sample-cache reuse is disabled. CRFty's qualified History cache remains the owner of reuse. The upstream library discussion can continue independently; its acceptance does not block this integration.

### Consequences

* Good: CRFty has no ab-av1 Cargo dependency, private revision, fork, or lifecycle patch.
* Good: An operation's internal runtime exits with its CLI process.
* Bad: Search exposes completed trials rather than rich per-sample progress.
* Bad: Individual internal probes cannot be given CRFty's typed deadlines or capture limits through the official CLI.
* Bad: Windows layouts that override configured tools before PATH fail closed; explicit upstream tool paths would simplify this boundary.

## More information

[Operation ownership](021-own-media-operation-processes.md), [tool installation](023-require-a-user-supplied-ffmpeg-toolchain.md), and [process boundary details](../design/ab-av1-process-boundary.md).
