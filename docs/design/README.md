# Design Notes

Long-form design and research for the rewrite. Content that would otherwise bloat a GitHub issue lives here as a versioned, greppable document; the issue links the doc.

## Where knowledge lives

- `docs/*.md`: settled behavior contracts, stating verified behavior only.
- `docs/design/*.md`: working research and design notes, including material that is not yet settled.
- `docs/adr/*.md`: decision records, one decision each, immutable once accepted.

## Header

Every document opens with a header block: purpose and boundary, status, and owning issue.

## Lifecycle

Each document is owned by exactly one issue, named in the document header and linked from that issue's body. No document is shared between issues, and none is orphaned.

A design document is working material, not a permanent artifact. When its owning issue closes, distill it: decisions become ADRs, settled behavior becomes a contract doc under `docs/`, and the design document is deleted. Leaving it in place creates a second, unmaintained account of a subject that is already covered elsewhere.

The exception is a stable reference document, such as a survey of prior art or a record of an external tool's observed behavior, whose value does not expire when an issue closes. Mark it `Status: stable reference` in the header and it survives issue closure.
