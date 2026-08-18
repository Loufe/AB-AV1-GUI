# Architecture Decision Records

CRFty uses [MADR](https://adr.github.io/madr/) for decisions that affect multiple modules, choose between competing architectures, establish lasting conventions, or would be costly to reverse.

Do not create ADRs for contained implementation details, routine refactors, or bug fixes.

## Naming and maintenance

- Store ADRs in `docs/adr/` as `NNN-short-title.md`.
- Use lowercase, hyphenated, present-tense imperative titles.
- Number records sequentially.
- Every committed ADR is accepted. Do not use a separate proposal or acceptance lifecycle.
- Treat ADRs as living records of the current architectural truth. When a decision changes, edit the existing ADR in place so its context, options, outcome, consequences, and references describe the new truth.
- When an ADR no longer represents a current decision, delete it and remove every reference to it from code comments and documentation in the same change. Git history preserves the old record; do not keep deprecated, superseded, or tombstone ADRs.
- Record one decision per ADR and link related records.

## Template

```markdown
---
status: accepted
date: YYYY-MM-DD
---

# Short Title

## Context and Problem Statement

Describe the decision and why it is needed.

## Decision Drivers

* Driver

## Considered Options

* Option

## Decision Outcome

Chosen option: **Option**, because reason.

### Consequences

* Good: Benefit
* Bad: Accepted trade-off

## More Information

Link related ADRs, research, issues, or implementation changes.
```
