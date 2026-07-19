# Architecture Decision Records

CRFty uses [MADR](https://adr.github.io/madr/) for decisions that affect multiple
modules, choose between competing architectures, establish lasting conventions,
or would be costly to reverse.

Do not create ADRs for contained implementation details, routine refactors, or
bug fixes.

## Naming and lifecycle

- Store ADRs in `docs/adr/` as `NNN-short-title.md`.
- Use lowercase, hyphenated, present-tense imperative titles.
- Number records sequentially.
- Use `proposed`, `accepted`, `deprecated`, or `superseded by NNN` status.
- Keep accepted ADRs immutable. Supersede rather than rewrite them.
- Record one decision per ADR and link related records.

## Template

```markdown
---
status: proposed | accepted | deprecated | superseded by NNN
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
