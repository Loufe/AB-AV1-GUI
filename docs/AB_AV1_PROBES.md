# Media probe boundaries

CRFty directly supervises its media inspection and session-start capability probes with bounded stdout, bounded stderr, concurrent readers, and per-invocation deadlines. Cancellation stops their process group or Windows Job before the terminal report is published.

Official ab-av1 is an external CLI. Its internal FFprobe invocations and local input sniff remain upstream implementation details: CRFty does not impose an individual timeout or memory-capture limit on them and does not expose the former patched library's probe-error categories. Stopping the outer CLI cancels that whole operation, including a blocked internal read. See the [process boundary](design/ab-av1-process-boundary.md).

The session-start synthetic quality search has an outer deadline and verifies the required JSON contract against the selected executable. Its timeout, launch, and supervision failures remain typed capability failures. Ordinary quality searches have cooperative cancellation without a fixed deadline.

Real-process contracts cover invalid and oversized JSON, missing or conflicting terminal results, noisy concurrent output, cancellation, inherited-pipe descendants, reader failure, and reuse after cancellation or an injected telemetry panic. Real-media contracts exercise official ab-av1 0.11.7 and a real FFmpeg build, including configured tool paths with spaces and Unicode. The [containment contract](PROCESS_CONTAINMENT.md) states the platform limits.
