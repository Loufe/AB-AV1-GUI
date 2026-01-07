# Architecture Decision Records

This project uses [MADR](https://adr.github.io/madr/) (Markdown Any Decision Records) for documenting architectural decisions.

## When to Create an ADR

Create an ADR when making decisions that:
- Affect multiple modules or the overall system structure
- Choose between competing approaches, libraries, or patterns
- Establish conventions that future code should follow
- Are difficult to reverse without significant rework

Do NOT create an ADR for:
- Bug fixes or minor refactors
- Implementation details contained within a single module
- Obvious choices with no meaningful alternatives

## ADR Location and Naming

- **Location**: `docs/adr/`
- **Naming**: `NNN-short-title.md` (e.g., `001-use-svt-av1-encoder.md`)
- Use lowercase with hyphens, present-tense imperative verbs
- Number sequentially starting from 001

## Template

```markdown
---
status: proposed | accepted | deprecated | superseded by NNN
date: YYYY-MM-DD
---

# [Short Title]

## Context and Problem Statement

[2-3 sentences describing the situation requiring a decision]

## Decision Drivers

* [Driver 1]
* [Driver 2]

## Considered Options

* [Option 1]
* [Option 2]

## Decision Outcome

Chosen option: "[Option N]", because [justification].

### Consequences

* Good: [positive impact]
* Bad: [trade-off accepted]

## More Information

[Optional: links to related ADRs, research, or implementation PRs]
```

## Rules

1. **One decision per ADR** - Keep records focused
2. **Immutable once accepted** - Create new ADRs to supersede, don't edit accepted decisions
3. **Status lifecycle**: `proposed` → `accepted` → optionally `deprecated` or `superseded by NNN`
4. **Include rejected options** - Document what was considered and why it wasn't chosen
5. **Link related ADRs** - Reference prior decisions that informed this one

## Referencing ADRs

When implementing code based on an ADR, reference it in comments only where the connection isn't obvious:

```python
# See ADR-003 for queue priority algorithm rationale
```
