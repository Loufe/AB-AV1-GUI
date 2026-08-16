# V3 History: environment, execution, and runtime collection research

Status: living research note; not an accepted design or implementation specification

Owning issue: [#94](https://github.com/Loufe/AB-AV1-GUI/issues/94)

Related issues: [#92](https://github.com/Loufe/AB-AV1-GUI/issues/92), [#93](https://github.com/Loufe/AB-AV1-GUI/issues/93), [#95](https://github.com/Loufe/AB-AV1-GUI/issues/95), [#96](https://github.com/Loufe/AB-AV1-GUI/issues/96), [#97](https://github.com/Loufe/AB-AV1-GUI/issues/97), [#99](https://github.com/Loufe/AB-AV1-GUI/issues/99), [#102](https://github.com/Loufe/AB-AV1-GUI/issues/102), [#103](https://github.com/Loufe/AB-AV1-GUI/issues/103)

## Purpose and boundary

This note synthesizes the investigation behind issue #94: which platform, execution, and runtime facts might make History and estimation more truthful, which operating-system sources can provide them, and what the available evidence establishes about their semantics and suitability.

It does not select physical tables, finalize the History observation model, or authorize the collectors in issue #99. Those decisions depend on the consumer and collection budgets from #92, the evidence-quality contract from #93, the sample/content research from #95, and the logical model from #96. Two findings recorded here are decided elsewhere as well: managed FFmpeg artifact durability belongs to #102 and Windows group-spawn failure cleanup to #103.

The labels below mean:

- **candidate required**: structurally necessary to interpret an observation; still subject to #92 and #93.
- **candidate best effort**: promising and inexpensive, but absence must be a normal typed result.
- **experimental**: retain only if fixtures and held-out evaluation prove value.
- **reject for initial production**: poor portability, attribution, privacy, or cost currently outweighs demonstrated value.

## Conclusions

1. Execution identity is a more immediate estimator gap than exotic hardware telemetry. The current estimator groups only by operation, codec, and resolution bucket even though `JobSpec` already freezes preset, sampling, decoder, target ladder, and exact ab-av1, FFmpeg, and encoder revisions. Mixing materially different settings and tool behavior should be measured before adding temperature, clock, or device inventory.
2. Runtime evidence must bind to the actual process attempt. A phase total is not enough: both search and encode can retry from hardware decode to software decode, and the current `PhaseTracker` intentionally merges re-entry into the same phase. Attaching CPU, memory, or I/O to `PhaseSpan` would therefore misattribute failed work to the successful attempt.
3. Keep both attempt cost and end-to-end user-wait cost. A clean successful attempt is useful for throughput modeling; the full run envelope, including retries and cleanup, is the truthful observation for queue-time prediction.
4. Separate requested execution intent from encoder-confirmed effective configuration. V3's typed arguments are authoritative for what it requested, but a representative managed-family SVT build mapped requested presets 12 and 13 to effective preset 11. SVT's public API has setters but no configuration getter, and FFmpeg does not receive normalized settings back. An estimator must not split or label cohorts by settings the encoder did not actually apply.
5. Use FFmpeg/SVT self-reporting for progress, runtime library identity, and effective encoder semantics, not as the preferred resource-accounting layer. `-benchmark` is a useful fixture oracle, but its timing excludes FFmpeg startup/teardown, startup failures can emit memory without timing, V3 hard-kill cancellation emits nothing, and its memory meaning differs by platform.
6. Reliable attempt resource counters belong at the process wait/reap or Job boundary, with explicit scope. Linux terminal `wait4` matched the FFmpeg leader's maximum RSS exactly in ten trials while covering more of that process lifecycle, but it is not a process-group aggregate. Windows Job accounting covers the contained tree, which matters when PATH launchers add a shim before FFmpeg. PID polling can miss short attempts and risks reuse. `sysinfo` remains a controlled oracle, not the preferred durable source.
7. Similar names do not imply comparable measurements. Windows Job Object I/O counts all I/O operations; Linux `/proc/<pid>/io` distinguishes characters passed through I/O calls from storage-layer bytes. Windows peak job memory and Linux maximum resident set have different semantics. Preserve the source-specific meaning rather than forcing both into a misleading portable field.
8. PSI and whole-system load do not isolate external contention. A highly parallel encoder creates CPU pressure itself. Store source counters, if selected, and version any derived contention signal; do not label PSI as "background load."
9. No environment fingerprint should be generated. A pathless export is pseudonymous, not guaranteed anonymous, and combinations of ordinary technical facts may still be distinctive. Persist only allowlisted values, disclose linkability, and never serialize raw collector objects.
10. Power, temperature, frequency, and accelerator utilization remain experimental or rejected for initial production. They are vendor- and platform-specific, frequently system-wide rather than job-attributable, and introduce native APIs or device identifiers without demonstrated predictive value.

## What V3 already knows

The rewrite already records more execution identity than the estimator uses:

- `ExecutionSettings`: requested VMAF target, fallback floor/step, overwrite rule, and requested decoder preference.
- `AnalysisProfile`: preset, maximum encoded percent, sample count and duration, thorough mode, actual analysis decoder, and exact ab-av1, FFmpeg, and encoder revisions.
- `CompletionEvidence::LiveEncode`: settled input/output byte sizes and the decoder actually used by the successful encode.
- `ConversionRun`: immutable job specification, analysis, outcome, wall-clock timestamps, and monotonic phase spans.

The revision fields are not yet equally authoritative. A managed archive checksum fixes the complete FFmpeg/SVT artifact, so its build identifier is a sound compatibility key even though it is not the literal SVT version. For system and explicit tools, discovery runs only ffprobe's JSON `-show_program_version` probe and assigns that one program version to both `ffmpeg_revision` and `encoder_revision`. FFmpeg and ffprobe may resolve from different tiers or builds, and a dynamically linked SVT library may change without either program version changing. Those fields are therefore proxies that can permit a stale analysis cache hit; they must not be described as exact runtime revisions until the executed FFmpeg and encoder are independently verified.

The current `EstimationModel` reduces eligible history to phase-time per second of video and groups it by codec and resolution bucket. It does not distinguish preset, tool revision, decoder mode, fallback work, available CPU, or resource limits. This is the relevant baseline for evaluating additional evidence.

Two existing fallback paths make attempt identity necessary:

- A non-quality hardware search failure restarts the full VMAF ladder in software and discards the hardware ladder's recorded quality attempts.
- A hardware encode failure recreates staging and retries once in software; terminal completion evidence records only the successful decoder.

`PhaseTracker` correctly preserves total user-wait time by merging repeated Analyzing or Encoding entries. It cannot also serve as an attempt ledger.

## Evidence layers and availability

The evidence separates into three storage-independent layers.

### Execution identity

Facts fixed before work begins and therefore eligible for cohort selection at claim time:

- application revision and collector contract version;
- platform family and architecture;
- exact effective execution settings and exact tool revisions;
- actual analysis decoder once resolved;
- available-parallelism estimate and any explicitly observed resource cap.

These facts may be used to select historical peers for the current estimate.

### Attempt evidence

One record per real search, encode, or remux process attempt:

- attempt ordinal and kind;
- requested and actual decoder/accelerator role;
- effective settings and tool identity;
- terminal outcome;
- monotonic wall duration;
- source-scoped process/job CPU, memory, I/O, fault, or context-switch counters;
- collector observation or typed absence.

Attempt evidence must survive failed and fallback attempts even when their quality result is not reusable. Error diagnostics remain separately scrubbed; raw command lines and environment variables are never attempt evidence.

### Run envelope

The complete claimed-job experience:

- current phase spans and total monotonic duration;
- all attempts, including failed fallback work;
- claim-time and terminal system snapshots when selected;
- cleanup, output settlement, and final outcome.

The envelope is the candidate input for predicting how long the user waits. Attempt evidence is the candidate input for process throughput and quality eligibility. Both are legitimate; substituting one for the other is not.

### Stage leakage rule

A fact may train or adjust a prediction only if the same fact is available at the prediction stage in production:

- Basic Scan cannot use terminal CPU, peak memory, I/O, or end-of-run PSI from the file being estimated.
- Claim-time load can be evaluated for claim-time queue estimates or live ETA.
- Terminal counters can filter, weight, or explain historical samples without becoming same-run Basic Scan features.
- Analysis results and sample evidence may improve Convert estimates only after analysis has actually completed.

## Candidate classification

| Candidate | Initial class | Stage | Source/semantic requirements | Main limitation |
| --- | --- | --- | --- | --- |
| Requested and effective execution settings | candidate required | prepared/attempt | Existing typed `JobSpec` for requested intent; a revision-scoped compatibility contract, preflight rejection, or encoder-confirmed effective value when coercion is possible | Estimator cohort explosion; accepted numeric range does not prove one-to-one behavior, and human startup-log parsing is not an ideal authority |
| Exact app and tool revisions | candidate required | prepared | App build identity; managed artifact checksum/contract; executed FFmpeg identity and runtime SVT version for external tools | Current system/explicit discovery copies ffprobe's version into both FFmpeg and encoder fields; never infer equivalence from version strings |
| Platform family and architecture | candidate required | startup/prepared | Typed enum such as Windows/Linux and x86_64/aarch64 | Raw kernel/build strings can be high-cardinality or contain custom suffixes |
| Actual decoder/accelerator role | candidate required | attempt | Existing `DecodeMode`, expanded to failed attempts | Hardware capability is not proof that hardware was actually used |
| Monotonic attempt and run wall time | candidate required | attempt/terminal | `Instant`-based duration; wall clock only for display chronology | Recovery cannot fabricate missing monotonic evidence |
| FFmpeg terminal self benchmark | experimental validation oracle; reject as initial durable collector | attempt terminal | FFmpeg `-benchmark`: transcode real/user/system time and platform-specific maximum memory | Partial on startup failure, absent on hard kill, human log, process-only scope, and lifecycle/semantic mismatch with terminal OS accounting |
| SVT runtime self-report | candidate best effort; required when supported settings can be coerced and no revision contract exists | attempt start | Allowlisted runtime version, selected instruction level, parallelism, warnings, and effective configuration from SVT startup output | Human/version-dependent log; SVT exposes no structured effective-configuration getter through FFmpeg |
| Available parallelism | candidate best effort | startup/prepared | `std::thread::available_parallelism`, with source and approximation quality | Can over/undercount affinity, job, VM, or cgroup limits; do not call in hot loops |
| CPU quota/cpuset or Job limits | candidate best effort | prepared | Linux cgroup hierarchy; Windows containment/parent limits where safely observable | The owned Windows inner Job does not expose an ancestor's effective limits; Linux likewise requires hierarchy traversal |
| Process/job user and kernel CPU | candidate best effort | attempt terminal | Linux `wait4` leader plus descendants it waited for; Windows Job Object accounting for the contained tree | Linux conditional lineage semantics differ from Windows job-tree semantics |
| Peak memory | candidate best effort, source-specific | attempt terminal | Linux leader/waited-lineage `ru_maxrss` in KiB; Windows Job Object peak memory | Linux reports the largest process RSS rather than aggregate tree memory; Windows uses different job memory/commit semantics |
| Process/job I/O | Windows candidate; Linux terminal source rejected on current evidence | attempt terminal | Windows Job Object `IO_COUNTERS`; Linux `wait4` block-operation counts only | WSL fixture returned `EACCES` for exited-unreaped `/proc/<pid>/io`; Windows all-I/O bytes and Linux block operations are not equivalent |
| Page faults/context switches | experimental | attempt terminal | `rusage` or Job Object counters when available | Predictive value unknown and cross-platform sets differ |
| Claim-time system CPU/memory | experimental | claim | Linux `/proc/stat` and `/proc/meminfo`; safe Windows wrapper | Snapshot is noisy; a delta needs a defined interval and cannot explain cause |
| PSI deltas | experimental, Linux only | claim/run envelope | `/proc/pressure/*` or cgroup v2 `*.pressure` | Encoder self-contention contributes; cgroup may contain unrelated processes |
| Derived external-busy estimate | experimental | terminal/live | Versioned derivation from system CPU minus job CPU and capacity | Counter scopes/capacity may not match; clamp artifacts need disclosure |
| CPU topology/SIMD/performance class | experimental | startup | Coarse allowlisted capabilities or a separately approved calibration | Model/brand/topology combinations increase linkability; predictive gain unproven |
| Virtualization class | experimental | startup | Coarse native/VM/container/WSL class only | Detection is incomplete and environment class may proxy unrelated differences |
| Accelerator inventory/driver | experimental | startup | Coarse vendor/capability only; actual decoder remains authoritative | Runtime availability differs from build support; device identity is prohibited |
| Temperature/frequency/power/throttle | reject for initial production | sampled/run | Vendor/platform-specific sysfs, APIs, or libraries | Poor attribution, native API burden, permissions, device identity, and no proven gain |
| Raw hardware/host/process identity | prohibited | never | None | Serial, UUID, instance, bus address, account, host/user, PID/PGID, cgroup path, and raw command line are outside the contract |

"Required" does not mean every platform call must succeed. It means the logical fact or its explicit absence is necessary to interpret the observation.

## Evidence representation constraints

Issue #93 owns the final evidence types. Missing values must remain explicit rather than becoming zero, free-form operating-system errors must remain outside the portable core, and operational diagnostics must be bounded and privacy-scrubbed. Units and names must preserve source semantics; for example, Linux `max_resident_kib` and Windows `job_peak_memory_bytes` must not be collapsed into a generic peak-memory field, nor should storage-layer bytes and all-I/O transfer bytes share one name.

## FFmpeg, SVT, and supervisor collection boundary

FFmpeg and SVT are the best sources for facts about the media pipeline and encoder-effective behavior. They are not the most complete source for attempt lifecycle resource usage.

| Fact | Preferred source | Contract treatment |
| --- | --- | --- |
| Requested encoder/search settings | V3's typed request and pinned ab-av1 adapter arguments | Authoritative intent; do not reconstruct by parsing logs |
| Encoder-effective settings | Revision-scoped compatibility contract or preflight validation; otherwise allowlisted SVT startup evidence | Record separately from requested intent; never assume equality when the encoder can map, clamp, or reject |
| Managed FFmpeg/encoder identity | Checksummed vendor manifest and discovered revisions | Authoritative only while the exact managed artifact remains obtainable; [#102](https://github.com/Loufe/AB-AV1-GUI/issues/102) owns artifact durability |
| Explicit/override runtime identity | Allowlisted `ffmpeg -version` fields plus SVT startup version when emitted | Corroborating evidence with typed absence; never retain the complete banner/build configuration |
| Encode/search progress | Existing typed ab-av1 adapter updates; FFmpeg `-progress` for direct FFmpeg operations such as remux | Operational machine-readable source; retain durable fields only for a named History consumer |
| Process/job CPU and memory | Linux terminal `wait4` for the FFmpeg leader/waited lineage; Windows Job accounting for the contained tree | Preferred source-qualified attempt evidence because it survives leader hard kill and avoids a human benchmark parser; never imply equal scope |
| FFmpeg self benchmark | Synthetic validation oracle | Do not persist initially; it is partial/absent on important outcomes and its timing scope differs from terminal accounting |
| Exit cause, cancellation escalation, retries, and decoder fallback | V3 supervisor/attempt ledger | Authoritative; FFmpeg cannot see the enclosing orchestration |
| Resource caps and system contention | Narrow OS collector only if a consumer justifies it | Not replaceable by an FFmpeg query |

FFmpeg documents [`-progress`](https://ffmpeg.org/ffmpeg.html) as periodic machine-readable `key=value` sequences ending in `progress=continue` or `progress=end`. V3's remux path already uses this interface. Search and encode already receive typed progress from the pinned ab-av1 adapter, so History should not introduce a second competing progress parser.

FFmpeg's [`-benchmark`](https://ffmpeg.org/ffmpeg.html) prints transcode real, system, and user time plus maximum memory. [Current FFmpeg source](https://ffmpeg.org/doxygen/trunk/ffmpeg_8c_source.html) starts the timing baseline after option parsing and input/output initialization, uses `getrusage(RUSAGE_SELF)` and `ru_maxrss` on Linux, and uses `GetProcessTimes` plus `PROCESS_MEMORY_COUNTERS.PeakPagefileUsage` on Windows. The memory value is process-lifetime cumulative, but the time values omit startup and teardown. Linux maximum resident set and Windows peak pagefile usage must never share one portable field.

### FFmpeg/SVT lifecycle fixture

All workloads were synthetic lavfi inputs with null outputs. No media path, process ID, command line, host/user name, or device identity was retained. The temporary scripts, archive, and extracted binary were removed after the fixture.

The Windows fixture used FFmpeg 8.1.2. Five successful trials emitted two lines: real/user/system time followed by `maxrss`. Two failures during input initialization or encoder selection emitted only `maxrss`; a parser must therefore support partial terminal evidence rather than one atomic benchmark record.

The Linux fixture used a retained BtbN FFmpeg 8.1 build from the same release family as V3's pin, with its published SHA-256 verified before extraction. It is not the exact pinned build: BtbN had already deleted V3's July 19 daily artifact under its [documented retention policy](https://github.com/BtbN/FFmpeg-Builds#release-retention-policy). The deletion reaches past this fixture: fresh managed installs and cache-miss media-contract CI runs now fail outright, because the pinned archive they resolve is gone. [Issue #102](https://github.com/Loufe/AB-AV1-GUI/issues/102) owns that separate managed-install failure.

On the Linux fixture, normal SVT encodes emitted both benchmark lines. `SIGINT` and `SIGTERM` allowed FFmpeg to emit full benchmark output; `SIGKILL` emitted none. The pinned ab-av1 fork calls `AsyncGroupChild::kill`, which [`command-group` 5.0.1 documents](https://docs.rs/command-group/5.0.1/command_group/struct.AsyncGroupChild.html#method.kill) and implements as process-group `SIGKILL` on Unix and `TerminateJobObject` on Windows. Therefore stopped/cancelled V3 attempts cannot rely on FFmpeg benchmark evidence.

Across five one-frame and five thirty-frame Linux SVT trials, FFmpeg `maxrss` exactly matched the parent-observed terminal maximum RSS in every trial (91,800-92,680 KiB and 147,620-150,456 KiB respectively). Parent-observed CPU and wall time were slightly larger because terminal accounting included the lifecycle outside FFmpeg's transcode baseline. The self benchmark adds no Linux peak-memory fact and provides a narrower time scope than `wait4`.

The Windows `ffmpeg` command was a Chocolatey launcher shim, confirmed through non-identifying executable metadata. Sampling that leader handle measured the shim rather than the descendant encoder, while FFmpeg's self-report came from the descendant. This is a concrete reason to use the already-owned Job tree rather than leader-PID polling; command launchers are not merely theoretical.

The representative BtbN build contained SVT-AV1 `v4.1.0-279-gd3c4cb394`. One-frame trials across V3's accepted preset range 0-13 showed requested presets 0-11 preserved, requested 12 mapped to effective 11, and requested 13 mapped to effective 11. The existing `MAX_ENCODING_PRESET = 13` and `AnalysisProfile.preset` therefore describe requested intent, not necessarily actual encoder configuration. The implementation must either reject/map unsupported settings before execution or retain separately sourced effective settings; an estimator must not treat requested 11, 12, and 13 as three different workloads when the active encoder executes all three as 11.

This is revision-sensitive behavior, not a stable meaning of the preset integer. [SVT-AV1 2.3.0](https://gitlab.com/AOMediaCodec/SVT-AV1/-/releases/v2.3.0) documented M12/M13 mapping to M11 while retaining the accepted API range; [SVT-AV1 3.0.0](https://gitlab.com/AOMediaCodec/SVT-AV1/-/releases/v3.0.0) repositioned presets and declared M10 the maximum unique preset; later releases added faster RTC-only modes. The current public header still enumerates M0-M13, so an accepted range or `ffmpeg -h encoder=libsvtav1` cannot establish that each number selects a distinct workload.

SVT 4.1's [public encoder header](https://gitlab.com/AOMediaCodec/SVT-AV1/-/raw/v4.1.0/Source/API/EbSvtAv1Enc.h) exposes `svt_av1_enc_init_handle`, `svt_av1_enc_set_parameter`, and `svt_av1_enc_parse_parameter`, but no configuration getter; its stream-info API exposes pass statistics rather than normalized configuration. FFmpeg 8.1.2's [libsvtav1 wrapper](https://raw.githubusercontent.com/FFmpeg/FFmpeg/n8.1.2/libavcodec/libsvtav1.c) fills a local configuration, passes it to `svt_av1_enc_set_parameter`, and then initializes the encoder without reading effective values back. SVT 4.x adds a global log callback for direct library consumers, but FFmpeg does not register it and callback output remains diagnostic text rather than a typed normalized-config result. V3 therefore cannot query effective SVT configuration through FFmpeg today.

For managed artifacts, the least ambiguous contract is a CI-produced, exact-revision compatibility table derived from tiny synthetic encodes and bound to the vendor manifest checksum. Preparation can reject settings outside the proven one-to-one set or deliberately canonicalize them before they reach History. A system override must pass the same behavioral probe or provide typed absence and be excluded from setting-sensitive cohorts; matching only a version string or advertised option range is insufficient. Startup-log parsing is a corroborating fallback, not the primary contract.

The same privacy-safe synthetic encode can close the external-tool identity gap: invoke the selected FFmpeg directly with a lavfi source and null output, retain only an allowlisted FFmpeg version, SVT runtime version, requested preset, and effective preset, then discard all raw output. This proves the executable/encoder pair actually used and avoids real media paths. It should be cached only ephemerally for the session and requested configuration unless a separate stable external-artifact identity is defined; a persisted ffprobe version cannot prove that a dynamically linked encoder stayed unchanged.

The pinned ab-av1 parser is not currently shaped for benchmark collection. `FfmpegOutStream` reads arbitrary 4 KiB stderr chunks, retains a bounded tail, and attempts one parse against only the latest non-empty line per chunk. FFmpeg emits timing and memory as separate adjacent lines that may coalesce into one chunk, and the stream exposes no terminal resource report. Adding a reliable benchmark path would require terminal parsing, partial-field handling, error attachment, and parser-version fixtures, not just adding `-benchmark` to `enc_args`. That integration cost is unjustified while terminal OS accounting is more complete.

`-benchmark_all` remains rejected: it expands unstable human diagnostic parsing without a named consumer. FFmpeg [`-report`](https://ffmpeg.org/ffmpeg.html) is prohibited because it writes the complete command line and log. Ordinary stderr can also contain media paths and metadata, so any SVT runtime parser must emit only typed allowlisted fields and discard raw lines.

## Platform source review

### Portable Rust and `sysinfo`

[`available_parallelism`](https://doc.rust-lang.org/std/thread/fn.available_parallelism.html) is a cheap, safe estimate of default parallel capacity, not a topology or current-load API. Its own documentation records Windows Job Object and affinity overcounting, Linux affinity/cgroup failure modes, VM overcommit, and the cost of cgroup-v1 mount scans. Persist the value with approximate quality and the collector version, never as "logical CPU count."

[`sysinfo` 0.39](https://docs.rs/sysinfo/latest/sysinfo/struct.ProcessRefreshKind.html) offers safe, targeted CPU, memory, and disk refreshes. Its relevant configuration is `System::new()` with narrow refresh kinds, not `System::new_all()` or all-process enumeration. On Linux, "everything" can traverse every task; on Windows, `Process::disk_usage` represents all I/O rather than Unix storage-layer I/O. CPU percentage also needs retained prior state and a sampling interval, while `accumulated_cpu_time` is a cumulative CPU-millisecond counter.

The evidence supports only these roles for `sysinfo`:

- a candidate safe wrapper for claim-time system CPU/memory facts;
- an oracle for comparing a lower-level attempt collector;
- a fallback only if sampling error and missed short attempts are measured and accepted explicitly.

It must never refresh user, cwd, executable path, command line, environment, network interfaces, motherboard, or product identity for this feature.

### Windows

`command-group` already creates a Job Object for each spawned contained process. Windows Job Objects include child processes by default and preserve accounting for terminated members. Microsoft documents aggregate user/kernel time and process counts in [`JOBOBJECT_BASIC_ACCOUNTING_INFORMATION`](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_accounting_information), all-process I/O counters in [`JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION`](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_and_io_accounting_information), and peak job memory in [`JOBOBJECT_EXTENDED_LIMIT_INFORMATION`](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information).

This is the strongest current process-tree source, but `command-group` 5.0.1 keeps the Job handle private and exposes no resource snapshot. Direct Win32 calls from `crfty-engine` would require first-party `unsafe`, conflicting with ADR-005. The safe extension point is therefore a narrow resource snapshot in a reviewed dependency or fork, not scattered `windows-sys` calls in the engine. The snapshot must occur before the Job handle closes and must not expose the handle or member PIDs.

Nested Job limits need separate treatment. Parent limits influence descendants, and CPU-rate quotas are relative through the hierarchy. Querying the child Job created by `command-group` may not reveal the full effective outer constraint. `available_parallelism` likewise documents that it may overcount Job-limited capacity. Record "not observable" rather than pretending the child limit is the effective limit.

Whole-system CPU deltas are feasible through [`GetSystemTimes`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getsystemtimes), but systems with more than 64 processors require processor-group care. System memory is available through standard memory APIs or a safe wrapper. These facts remain experimental until a same-stage estimator experiment shows value.

### Linux

The terminal CPU source is [`wait4`](https://www.man7.org/linux/man-pages/man2/wait4.2.html) plus [`rusage`](https://man7.org/linux/man-pages/man2/getrusage.2.html). Linux reports user/system CPU, maximum resident set in KiB, faults, block-operation counts, and context switches. Some `rusage` members are unmaintained zeros, and `ru_maxrss` for `RUSAGE_CHILDREN` is the largest child rather than a process-tree peak. The collector must expose only documented maintained fields and identify its scope.

Linux [`getrusage`](https://www.man7.org/linux/man-pages/man2/getrusage.2.html) includes grandchildren and further descendants only when every intervening process waited for its children. `wait4` cannot reap an arbitrary grandchild merely because it shares the leader's process group; an unwaited descendant is reparented when its parent exits. Therefore the honest scope is the FFmpeg leader plus descendant usage that was folded into it by intervening waits, not the contained process group. This still captures SVT's encoder threads because they execute inside FFmpeg, but it cannot promise Windows-Job-equivalent tree accounting.

`command-group` 5.0.1 currently calls `waitpid` and discards resource usage. Its Unix loop passes the negative process-group ID and says it waits for the group completely, but `waitpid` can return only the caller's waitable children; V3 is not a child subreaper. A reviewed dependency change could use `wait4` and return a safe source-qualified terminal snapshot, but it must not label that snapshot as group aggregate. This is preferable to polling. A Linux-only experiment used `waitid(..., WNOWAIT)` to leave exited direct children waitable before reading `/proc/<pid>/io` and reaping them. Although the [`waitid` contract](https://man7.org/linux/man-pages/man2/waitpid.2.html) supports leaving the child waitable, the WSL fixture returned `EACCES` for the I/O file on successful, killed, and fast-exiting zombies. Terminal `/proc` I/O is therefore not a current recommendation; native/restricted Linux fixtures may explain portability, but cannot turn this failure into a required collector.

[`/proc/<pid>/io`](https://www.kernel.org/doc/html/latest/filesystems/proc.html) distinguishes characters passed through reads/writes from storage-layer bytes, has filesystem caveats, can account canceled writes, is permission-gated, and can tear on 32-bit systems. Persist only selected numeric counters. Never read or retain `cmdline`, `environ`, `cwd`, `exe`, UID/GID, or process names.

For limits and pressure, [`cgroup v2`](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html) provides `cpu.max`, `cpu.stat`, effective cpusets, memory limits/peaks, per-device I/O counters, and pressure files. Limits are hierarchical: a leaf default of `max` does not prove the workload is unconstrained by an ancestor. Resolving the current cgroup may require parsing mount and membership paths. Those paths are collector-internal routing data and must be discarded immediately, never journaled or exported. A shared cgroup's usage is not job usage.

[`PSI`](https://www.kernel.org/doc/html/latest/accounting/psi.html) reports time that tasks are stalled for CPU, memory, or I/O, system-wide or per cgroup. It is a contention signal, not an attribution signal. An encoder that deliberately keeps more work runnable than CPUs can raise CPU pressure itself. Evaluate raw counter deltas as an eligibility/weighting signal before trying a derived "external contention" feature.

Creating a dedicated cgroup per attempt could improve attribution but would add a new containment mechanism, privilege/delegation requirements, cleanup, and interaction with the existing process group. It is not an initial production candidate without a required consumer that simpler sources fail.

### Encoder and accelerator relevance

Exact encoder revision is essential. SVT-AV1's [`2.3.0` changelog](https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/CHANGELOG.md) changed `--lp` from a logical-processor target to a level of parallelism; even `--lp 1` can create multiple threads. A numeric setting without the encoder revision is not a stable execution identity.

FFmpeg's [`-hwaccels` documentation](https://ffmpeg.org/ffmpeg.html) explicitly separates build support from actual runtime availability. Therefore device inventory or advertised support cannot replace the actual decoder recorded on each attempt. The initial useful accelerator facts are the chosen decoder, its role (decode, not AV1 encode), and fallback outcome.

Vendor telemetry such as NVML exposes useful utilization, temperature, clocks, and power, but also serial numbers, PCI IDs, process IDs, and other prohibited identity in the same API surface. It is not justified without measured predictive benefit, a strict allowlist wrapper, equivalent AMD/Intel behavior, and a dependency/unsafe review.

### Power and throttling

Linux exposes some Intel thermal throttle counters and power/energy zones, but availability depends on CPU vendor, kernel drivers, sysfs permissions, and virtualization. RAPL energy is package- or zone-wide, not inherently attributable to one encode. Current frequency can differ from requested policy because of hardware coordination, thermal limits, and power limits. See the kernel's [`powercap`](https://www.kernel.org/doc/html/latest/power/powercap/powercap.html), [`thermal throttle`](https://www.kernel.org/doc/html/latest/admin-guide/thermal/intel_thermal_throttle.html), and [`CPU frequency`](https://www.kernel.org/doc/html/latest/admin-guide/pm/cpufreq.html) documentation.

Windows power policy similarly does not directly provide per-attempt thermal or power attribution. Until these measurements beat simpler execution identity and CPU-time normalization, initial production should omit them rather than ship a mostly-null, vendor-specific collector.

## Windows Job Object evidence

This fixture used a throwaway PowerShell/C# probe to call the documented Win32 APIs directly. It created an unnamed Job Object, assigned a synthetic child, waited for the child to exit, and only then queried accounting. No host, user, path, command-line, process ID, device, or Job handle value was printed or retained.

Environment:

- Windows build 10.0.26100, x64;
- 12 processors reported to the process environment;
- the initial accounting/overhead probe was not already inside an outer Job Object; a follow-up synthetic fixture created controlled outer Jobs for nested-limit behavior.

The direct-child workload touched a 64 MiB allocation, performed CPU work, and wrote/read/deleted a 4 MiB temporary file. Across five runs:

- accounting remained queryable after `WaitForExit`, with `ActiveProcesses = 0`;
- `TotalProcesses = 1`, user/kernel CPU and page-fault counts were nonzero;
- read transfer was about 4.49 MB and write transfer about 4.19 MB, illustrating that Job I/O includes process/runtime activity in addition to the explicit payload;
- peak process/job memory was 192.5-193.1 MB, not the allocation size, because the counter measures the complete process rather than the test payload;
- `TotalTerminatedProcesses = 0`. This field means processes terminated because of a Job limit violation, not ordinary exited processes, and must not be modeled as an exit count.

A second workload kept the parent allocation alive while spawning a synthetic descendant. The Job reported three associated processes and zero active after wait. Aggregate peak Job memory was 307,531,776 bytes while the largest-process peak was 193,609,728 bytes; aggregate CPU, faults, and I/O also increased. This proves that Job-level evidence captures materially different scope than leader PID polling. The fixture intentionally did not enumerate or retain which helper processes made up that count.

The C# helper measured native calls internally for 10,000 iterations, avoiding PowerShell loop overhead. Five direct-child trials produced:

| Operation | Minimum | Median | Maximum |
| --- | ---: | ---: | ---: |
| Query Job basic + I/O accounting | 0.600 µs | 0.616 µs | 0.618 µs |
| `GetSystemTimes` | 4.067 µs | 4.280 µs | 5.327 µs |
| `GlobalMemoryStatusEx` | 1.322 µs | 1.376 µs | 1.385 µs |

The follow-up fixture assigned the probe to an outer Job with `ActiveProcessLimit = 2`, then assigned its synthetic child to an otherwise unconstrained inner Job. Nested assignment succeeded. After the child tree exited, the outer Job reported three total processes and the inner Job reported two, matching Microsoft's [nested-Job accounting contract](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs) that parent accounting aggregates child Jobs while each Job accounts its own subtree.

Querying the inner Job returned zero limit flags and no active-process limit even though the outer Job reported flag `0x8` and limit `2`. The parent limit remains effective according to the Windows contract, but it is not surfaced by querying V3's inner Job handle. History must therefore represent ancestor constraint as unknown unless a separately authorized parent-discovery API proves it; the owned Job cannot truthfully report effective capacity by itself.

A second outer Job set UI restriction flag `0x1`. Nested assignment still succeeded on Windows build 10.0.26100 and the inner Job again accounted the child subtree, despite current Microsoft prose saying default nesting requires neither Job to set UI limits. Treat this as one platform observation, not a portability guarantee; assignment remains fallible and needs deterministic cleanup.

That cleanup is currently defective in `command-group` 5.0.1. Both [Windows spawn paths](https://github.com/watchexec/command-group/blob/v5.0.1/src/stdlib/windows.rs) create raw Job/completion-port handles through its [Job helper](https://github.com/watchexec/command-group/blob/v5.0.1/src/winres.rs), spawn the child suspended, and return immediately on `AssignProcessToJobObject` or thread-resumption error before an owning group wrapper exists. Rust child drop does not terminate/reap the process, and the raw handles have no error-path owner. [Issue #103](https://github.com/Loufe/AB-AV1-GUI/issues/103) owns the dependency fix and regression tests; resource-accounting exposure remains here and in #99.

This supports terminal-boundary collection rather than polling, but it does not yet prove that `command-group` can expose the snapshot without changing handle, wait, cancellation, or completion-port behavior. It also does not test CPU-rate limits, affinity restrictions, cancellation, very fast children, or restricted host Jobs whose handles V3 cannot access. The throwaway source was removed after recording the fixture.

## Linux and WSL accounting evidence

These measurements establish capability and read cost only; they are not a cross-platform collector benchmark.

Environment:

- Linux 6.18 WSL2 class, x86_64;
- 12 available processors reported by the environment;
- cgroup v2 mounted;
- system CPU, memory, and I/O PSI files readable;
- `/proc/self/stat`, `/proc/self/status`, and `/proc/self/io` readable;
- cgroup root exposed usage/pressure but no leaf `cpu.max` or `memory.max`, demonstrating that a collector cannot assume limit files at the mount root;
- no cpufreq policy or powercap zone exposed; thermal cooling devices alone did not provide a portable temperature/throttle measurement.

A throwaway optimized Rust program performed 10,000 cached open-and-read operations per trial for five trials. It did no parsing or serialization and printed no source contents.

| Operation | Minimum | Median | Maximum |
| --- | ---: | ---: | ---: |
| `std::thread::available_parallelism()` | 22.605 µs | 22.913 µs | 26.284 µs |
| Read `/proc/self/{stat,status,io}` bundle | 36.962 µs | 37.107 µs | 37.683 µs |
| Read `/proc/stat`, `/proc/meminfo`, and three PSI files | 63.950 µs | 65.247 µs | 65.959 µs |

These numbers only show that narrow cached reads are plausible at startup, claim, or terminal boundaries on this host. They do not establish production overhead, parsing cost, Windows cost, cold-cache behavior, correctness under process exit, or an acceptable polling interval. The throwaway source and binary were removed after measurement.

### Linux terminal/reap fixture

A second throwaway native fixture tested the lifecycle that a modified `command-group` would actually own. For each direct child, the parent called `waitid(P_PID, ..., WEXITED | WNOWAIT)`, attempted allowlisted `/proc` reads, then called `wait4` and checked that `/proc/<pid>` disappeared after reaping. Cases covered normal success, a child that waited for its own descendant, an immediate nonzero exit, and forced `SIGKILL`.

Results on this WSL2 kernel:

- `wait4` returned user/system CPU, maximum RSS, faults, block operations, and context switches in all four cases, including immediate exit and `SIGKILL`.
- The successful direct child reported about 400 ms user CPU, 87 ms system CPU, 67,340 KiB maximum RSS, and 8,192 output block operations after touching 64 MiB and writing 4 MiB.
- When that child waited for a descendant that touched 32 MiB, used CPU, and wrote another 2 MiB, the returned totals rose to about 807 ms CPU, 99,996 KiB maximum RSS, and 12,288 output block operations. This is evidence that waited-descendant usage can reach the leader's terminal `rusage` on this kernel, but it has not been reproduced on native Linux and does not establish a simultaneous process-tree RSS peak.
- `/proc/<pid>/io` returned permission denied after `waitid(WNOWAIT)` in every case. `/proc/<pid>/status` remained readable for the zombie but `VmHWM` was zero, so it did not recover terminal peak memory. The process directory disappeared immediately after `wait4` reaped it.

This materially narrows the Linux recommendation: `wait4` CPU/max-RSS/fault/block/context-switch fields remain candidates, while terminal `/proc` byte-I/O and `VmHWM` are not dependable collectors. Obtaining byte I/O would require active polling, task accounting, or a dedicated cgroup, each of which has more cost and scope complexity and lacks a demonstrated consumer.
