# crfty-engine

Owns external processes and filesystem I/O.

- Must not depend on Tauri; crfty-shell is the only Tauri-aware crate.
- Use the official ab-av1 CLI JSON search contract (ADR-003); no patched library, fork, or private revision. FFmpeg encoding uses its machine progress protocol.
- Never parse human-oriented process output as an application contract.
- Do not introduce a generic encoder trait until a second backend is implemented.
- Process behavior requires real-process contract tests in addition to unit tests.
- ab-av1/FFmpeg/ffprobe are user-supplied (ADR-023): resolved from `CRFTY_AB_AV1`/`CRFTY_FFMPEG`/`CRFTY_FFPROBE`, then Settings paths, then PATH, and verified by the session-start capability probe in `tools/probe.rs`. Never download tools. Missing tools put the app in degraded mode (media commands fail with `engine_unavailable`) — preserve this; never panic on missing tools.
