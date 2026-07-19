---
status: accepted
date: 2026-07-19
---

# Forbid First-Party Unsafe Rust

## Context and Problem Statement

CRFty is a long-lived desktop process that owns durable state while coordinating
untrusted media tools. Native FFmpeg bindings or scattered operating-system calls
would move memory-safety and lifetime risks into that process.

## Decision Drivers

* Keep first-party memory safety compiler-enforced
* Isolate process crashes and unsafe transitive code behind external tools
* Make dependency and platform risk visible during review
* Support Windows and Linux process-tree containment

## Considered Options

* Permit unsafe Rust wherever required
* Use native FFmpeg bindings
* Forbid unsafe in first-party crates and isolate a platform exception only if proven necessary

## Decision Outcome

Chosen option: **Forbid unsafe Rust in first-party crates**, because safe process and
filesystem APIs cover the planned architecture and FFmpeg does not need to share the
application address space.

If platform acceptance tests prove safe wrappers insufficient, one small platform
crate may supersede this decision with a documented safe interface and audited unsafe
blocks. Dependencies are locked, denied against policy, vetted, and inventoried.

### Consequences

* Good: First-party unsafe cannot enter unnoticed
* Good: Native media crashes remain outside the durable process
* Bad: Some platform containment approaches may require a separately reviewed exception

## More Information

See issue #33, sections 12 and 18.
