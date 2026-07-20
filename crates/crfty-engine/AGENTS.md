# crfty-engine

Owns external processes and filesystem I/O.

- Must not depend on Tauri; crfty-shell is the only Tauri-aware crate.
- ab-av1 is a pinned, narrowly patched library dependency in the adapter (ADR-003).
  Do not widen the patch surface or track upstream casually.
- Never parse human-oriented process output as an application contract.
- Do not introduce a generic encoder trait until a second backend is implemented.
- Process behavior requires real-process contract tests in addition to unit tests.
- FFmpeg/ffprobe resolve from PATH or `CRFTY_FFMPEG`/`CRFTY_FFPROBE`. Missing tools
  put the app in degraded mode (commands fail with `engine_unavailable`) — preserve
  this; never panic on missing tools.
