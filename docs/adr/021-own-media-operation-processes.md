---
status: accepted
date: 2026-10-11
---

# Own media operation processes

## Context and problem statement

The official ab-av1 CLI runs one operation per process and launches ordinary descendants that inherit its containment. Process isolation removes the need to finalize ab-av1's global registries inside the application, but CRFty still must stop descendants, drain pipes, and clean its workspace before publishing completion.

## Decision drivers

* Cancellation must reach the whole media operation.
* Leader exit must not leave inherited pipes or descendants running.
* Pipe readers and temporary files must have clear owners.
* Failed cleanup must preserve the initiating failure and its diagnostic.

## Considered options

* Delegate lifecycle guarantees to a patched ab-av1 library.
* Own each external operation through the existing process supervisor.
* Introduce platform sandboxes as the process boundary.

## Decision outcome

Chosen option: **own each external operation through the existing process supervisor**. CRFty creates a Unix process group or Windows Job around the official CLI or direct FFmpeg operation, reads both pipes concurrently, requests group termination at leader exit or cancellation, waits for the child, and joins the readers before publishing its report. The [containment contract](../PROCESS_CONTAINMENT.md) records platform limits.

Each search owns a private temporary directory passed as `--temp-dir`, uses it as the working directory, and cleans it after process settlement. Encoding writes to the coordinator's journaled staging path; output promotion and abandonment remain governed by ADR-020. Cancellation is sticky and scoped to one job. Reader failures trigger cancellation, and caught protocol-consumer panics follow the same cleanup path.

### Consequences

* Good: No dependency-global cleanup call or in-process ab-av1 task lifetime crosses the application boundary.
* Good: Pipe pressure and malformed machine output cannot bypass operation supervision.
* Bad: A Unix process group does not kill itself when the application is forcibly terminated.
* Bad: The published Windows wrapper's completion notification is not an independent proof that every descendant has exited.
* Bad: Sandboxing and stronger crash containment remain separate, long-term wishlist work.

## More information

[CLI boundary](003-invoke-the-official-ab-av1-cli.md), [output transactions](020-own-output-promotion-as-a-journaled-transaction.md), and [sandbox wishlist](../design/media-worker-sandbox.md).
