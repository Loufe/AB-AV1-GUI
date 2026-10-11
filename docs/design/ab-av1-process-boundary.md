# Ab-av1 process boundary

CRFty uses official ab-av1 0.11.7 as an external quality-search tool and FFmpeg directly for encoding. [ADR-003](../adr/003-invoke-the-official-ab-av1-cli.md) selects the protocol boundary; [ADR-021](../adr/021-own-media-operation-processes.md) defines operation ownership.

## Isolation and tool selection

A fresh ab-av1 process owns its internal Tokio runtime and registries. Its FFmpeg and ffprobe descendants inherit CRFty's group or Job. The adapter does not embed a library, call global finalizers, or depend on an upstream lifecycle patch.

The official CLI invokes FFmpeg and ffprobe by bare names. On Unix, private PATH aliases symlink the exact selected tools. On Windows, aliases launch a small native CRFty relay which forwards arguments and streams to the original absolute executables, preserving their executable-directory dynamic library lookup. ab-av1 itself stays at its original path. The relay is built alongside CRFty and must be installed next to the application executable.

Windows searches the application and system directories before inherited PATH. Conflicting FFmpeg or ffprobe executables in those locations are rejected before launching ab-av1; CRFty conservatively checks both system-directory views. Explicit paths must never silently select another binary. Adding `AB_AV1_FFMPEG` and `AB_AV1_FFPROBE` upstream would remove this relay and layout restriction when a release provides that contract. Such a contribution is independent of the larger upstream library discussion.

## Protocol and evidence

Quality search consumes `sample-encode-done`, `crf-search-done`, and `crf-search-error` NDJSON messages. Every line has a byte limit, values must be finite and valid, and terminal JSON must agree with exit status. A failed search retains its last completed trial for the quality-target ladder. Human stderr remains a bounded, scrubbed diagnostic rather than a parsed control protocol.

Search telemetry reports completed trials. It has no frame rate, current sample, or continuous completion fraction; the UI shows an indeterminate phase. Encoding uses FFmpeg's machine progress protocol, validates its terminal marker, and retains the existing throughput and output-position telemetry.

Search and encoding explicitly share encoder, preset, pixel format, keyframe interval, scene detection, and decoder settings. The CLI sample cache is disabled; History owns reuse. The selected ab-av1 executable content hash invalidates reuse when its inspected bytes change. It does not identify loaded libraries or prevent replacement of the executable after inspection.

## Probe limits

CRFty's direct media and capability probes keep their bounded captures and deadlines. The synthetic CLI capability search has an outer deadline. Ordinary searches and encodes have responsive cancellation and no fixed operation deadline, because valid operations can run for long periods.

The official CLI's individual internal probes and local input sniff are opaque. Their memory capture and individual timeout are not controlled by CRFty. A blocked local read remains cancellable by stopping the outer process, but this does not restore the old library's internal probe error categories or per-probe deadline.

## Sandboxing

[Media sandboxing](media-worker-sandbox.md) is a long-term, ultra-low-priority wishlist item. It does not block process isolation or the use of unpatched upstream releases.
