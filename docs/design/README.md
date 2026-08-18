# Design Notes

Long-form design and research for the rewrite. Decisions and the reasoning behind them live here as versioned, greppable documents; issues link the document rather than restating it.

## Where knowledge lives

- `docs/*.md`: settled behavior contracts, stating verified behavior only.
- `docs/design/*.md`: working research and design notes, including material that is not yet settled.
- `docs/adr/*.md`: living records of current decisions, one decision each.

## Header

Every document opens with a title, a status line, then purpose and boundary.

## Ownership and linking

Each document has one owning issue, recorded in that issue's body and not in the document. Any number of issues may link a document.

## Lifecycle

A working note is material for a decision in progress. When its subject settles, distill it: decisions become ADRs, settled behavior becomes a contract doc under `docs/`, and the note is deleted rather than left as a second unmaintained account of a subject covered elsewhere.

A working note may carry a register of recorded verdicts under stable labels. Register entries are durable even while the surrounding note is not, and they survive into the ADR or contract doc that replaces it.

A document whose value does not expire, such as a survey of prior art or a record of an external tool's observed behavior, is not working material. Mark it `Status: stable reference` and it persists.
