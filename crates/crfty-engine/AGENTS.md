# crfty-engine

Owns external processes and filesystem I/O.

- Must not depend on Tauri; crfty-shell is the only Tauri-aware crate.
- ab-av1 is a pinned, narrowly patched library dependency in the adapter (ADR-003). Do not widen the patch surface or track upstream casually.
- Never parse human-oriented process output as an application contract.
- Do not introduce a generic encoder trait until a second backend is implemented.
- Process behavior requires real-process contract tests in addition to unit tests.
- FFmpeg/ffprobe are user-supplied (ADR-023): resolved from `CRFTY_FFMPEG`/`CRFTY_FFPROBE`, then Settings paths, then PATH, and verified by the session-start capability probe in `tools/probe.rs`. Never download tools. Missing tools put the app in degraded mode (media commands fail with `engine_unavailable`) — preserve this; never panic on missing tools.
