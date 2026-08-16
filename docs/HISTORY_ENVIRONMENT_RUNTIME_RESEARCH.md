# V3 History: environment, execution, and runtime collection research

Status: living research note; not an accepted design or implementation specification  
Tracking issue: [#94](https://github.com/Loufe/AB-AV1-GUI/issues/94)  
Related decisions: [#92](https://github.com/Loufe/AB-AV1-GUI/issues/92), [#93](https://github.com/Loufe/AB-AV1-GUI/issues/93), [#95](https://github.com/Loufe/AB-AV1-GUI/issues/95), [#96](https://github.com/Loufe/AB-AV1-GUI/issues/96), [#97](https://github.com/Loufe/AB-AV1-GUI/issues/97), [#99](https://github.com/Loufe/AB-AV1-GUI/issues/99)  
Last updated: 2026-08-15

## Purpose and boundary

This note records the investigation behind issue #94: which platform,
execution, and runtime facts might make History and estimation more truthful,
which operating-system sources can provide them, and which experiments still
have to be run before selecting production collectors.

It does not select physical tables, finalize the History observation model, or
authorize the collectors in issue #99. Those decisions depend on the consumer
and collection budgets from #92, the evidence-quality contract from #93, the
sample/content experiments from #95, and the logical model from #96.

The labels below mean:

- **candidate required**: structurally necessary to interpret an observation;
  still subject to #92 and #93.
- **candidate best effort**: promising and inexpensive, but absence must be a
  normal typed result.
- **experimental**: retain only if fixtures and held-out evaluation prove value.
- **reject for initial production**: poor portability, attribution, privacy, or
  cost currently outweighs demonstrated value.

## Current conclusions

1. Execution identity is a more immediate estimator gap than exotic hardware
   telemetry. The current estimator groups only by operation, codec, and
   resolution bucket even though `JobSpec` already freezes preset, sampling,
   decoder, target ladder, and exact ab-av1, FFmpeg, and encoder revisions.
   Mixing materially different settings and tool behavior should be measured
   before adding temperature, clock, or device inventory.
2. Runtime evidence must bind to the actual process attempt. A phase total is
   not enough: both search and encode can retry from hardware decode to software
   decode, and the current `PhaseTracker` intentionally merges re-entry into the
   same phase. Attaching CPU, memory, or I/O to `PhaseSpan` would therefore
   misattribute failed work to the successful attempt.
3. Keep both attempt cost and end-to-end user-wait cost. A clean successful
   attempt is useful for throughput modeling; the full run envelope, including
   retries and cleanup, is the truthful observation for queue-time prediction.
4. Reliable process CPU and terminal resource counters belong at the process
   wait/reap boundary. PID polling can miss short attempts, cannot inspect a
   process after it has been reaped, and risks PID reuse. `sysinfo` is useful as
   a safe portability oracle and for controlled experiments, but is not yet the
   preferred source for durable per-attempt accounting.
5. Similar names do not imply comparable measurements. Windows Job Object I/O
   counts all I/O operations; Linux `/proc/<pid>/io` distinguishes characters
   passed through I/O calls from storage-layer bytes. Windows peak job memory
   and Linux maximum resident set have different semantics. Preserve the
   source-specific meaning rather than forcing both into a misleading portable
   field.
6. PSI and whole-system load do not isolate external contention. A highly
   parallel encoder creates CPU pressure itself. Store source counters, if
   selected, and version any derived contention signal; do not label PSI as
   “background load.”
7. No environment fingerprint should be generated. A pathless export is
   pseudonymous, not guaranteed anonymous, and combinations of ordinary
   technical facts may still be distinctive. Persist only allowlisted values,
   disclose linkability, and never serialize raw collector objects.
8. Power, temperature, frequency, and accelerator utilization remain
   experimental or rejected for initial production. They are vendor- and
   platform-specific, frequently system-wide rather than job-attributable, and
   introduce native APIs or device identifiers without demonstrated predictive
   value.

## What V3 already knows

The rewrite already records more execution identity than the estimator uses:

- `ExecutionSettings`: requested VMAF target, fallback floor/step, overwrite
  rule, and requested decoder preference.
- `AnalysisProfile`: preset, maximum encoded percent, sample count and duration,
  thorough mode, actual analysis decoder, and exact ab-av1, FFmpeg, and encoder
  revisions.
- `CompletionEvidence::LiveEncode`: settled input/output byte sizes and the
  decoder actually used by the successful encode.
- `ConversionRun`: immutable job specification, analysis, outcome, wall-clock
  timestamps, and monotonic phase spans.

The current `EstimationModel` reduces eligible history to phase-time per second
of video and groups it by codec and resolution bucket. It does not distinguish
preset, tool revision, decoder mode, fallback work, available CPU, or resource
limits. That is the first baseline to challenge.

Two existing fallback paths make attempt identity necessary:

- A non-quality hardware search failure restarts the full VMAF ladder in
  software and discards the hardware ladder's recorded quality attempts.
- A hardware encode failure recreates staging and retries once in software;
  terminal completion evidence records only the successful decoder.

`PhaseTracker` correctly preserves total user-wait time by merging repeated
Analyzing or Encoding entries. It cannot also serve as an attempt ledger.

## Evidence layers and availability

The research should evaluate three storage-independent evidence layers.

### Execution identity

Facts fixed before work begins and therefore eligible for cohort selection at
claim time:

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

Attempt evidence must survive failed and fallback attempts even when their
quality result is not reusable. Error diagnostics remain separately scrubbed;
raw command lines and environment variables are never attempt evidence.

### Run envelope

The complete claimed-job experience:

- current phase spans and total monotonic duration;
- all attempts, including failed fallback work;
- claim-time and terminal system snapshots when selected;
- cleanup, output settlement, and final outcome.

The envelope is the candidate input for predicting how long the user waits.
Attempt evidence is the candidate input for process throughput and quality
eligibility. Both are legitimate; substituting one for the other is not.

### Stage leakage rule

A fact may train or adjust a prediction only if the same fact is available at
the prediction stage in production:

- Basic Scan cannot use terminal CPU, peak memory, I/O, or end-of-run PSI from
  the file being estimated.
- Claim-time load can be evaluated for claim-time queue estimates or live ETA.
- Terminal counters can filter, weight, or explain historical samples without
  becoming same-run Basic Scan features.
- Analysis results and sample evidence may improve Convert estimates only after
  analysis has actually completed.

## Candidate classification

| Candidate | Initial class | Stage | Source/semantic requirements | Main risk or unresolved question |
| --- | --- | --- | --- | --- |
| Effective execution settings | candidate required | prepared/attempt | Existing typed `JobSpec`; record requested and actual values | Estimator cohort explosion; decide exact-match and fallback ladder in #92/#93 |
| Exact app and tool revisions | candidate required | prepared | Existing discovered revisions plus app build identity | Decide grouping across revisions; never infer equivalence from version strings |
| Platform family and architecture | candidate required | startup/prepared | Typed enum such as Windows/Linux and x86_64/aarch64 | Raw kernel/build strings can be high-cardinality or contain custom suffixes |
| Actual decoder/accelerator role | candidate required | attempt | Existing `DecodeMode`, expanded to failed attempts | Hardware capability is not proof that hardware was actually used |
| Monotonic attempt and run wall time | candidate required | attempt/terminal | `Instant`-based duration; wall clock only for display chronology | Recovery cannot fabricate missing monotonic evidence |
| Available parallelism | candidate best effort | startup/prepared | `std::thread::available_parallelism`, with source and approximation quality | Can over/undercount affinity, job, VM, or cgroup limits; do not call in hot loops |
| CPU quota/cpuset or Job limits | candidate best effort | prepared | Linux cgroup hierarchy; Windows containment/parent limits where safely observable | Effective hierarchical limit is not always exposed by a leaf API |
| Process/job user and kernel CPU | candidate best effort | attempt terminal | Linux `wait4`/`rusage`; Windows Job Object accounting | Linux leader/process semantics differ from Windows job-tree semantics |
| Peak memory | candidate best effort, source-specific | attempt terminal | Linux `ru_maxrss` in KiB; Windows Job Object peak memory | Resident vs job memory/commit semantics are not portable |
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

“Required” does not mean every platform call must succeed. It means the logical
fact or its explicit absence is necessary to interpret the observation.

## Collector representation under test

Issue #93 owns the final evidence types. The #94 spike still needs one
serialization shape to prove that collectors cannot turn missing values into
zero or leak their raw source. A candidate fixture shape is:

```json
{
  "collector": "std.available_parallelism",
  "collector_version": 1,
  "source": "rust_std",
  "stage": "prepared",
  "status": {
    "kind": "collected",
    "value": 12,
    "unit": "parallel_units",
    "quality": "approximate"
  }
}
```

An unavailable observation is data, not zero:

```json
{
  "collector": "linux.cgroup_v2.cpu_quota",
  "collector_version": 1,
  "source": "cgroup_v2",
  "stage": "prepared",
  "status": {
    "kind": "unavailable",
    "reason": "not_exposed"
  }
}
```

Candidate absence reasons are `unsupported_platform`, `not_exposed`,
`permission_denied`, `process_exited`, `source_changed`, `parse_rejected`, and
`collector_failed`. Free-form OS error text must not enter the portable core.
The collector logs a bounded, privacy-scrubbed diagnostic separately when it is
operationally useful.

Units belong to the measurement contract. Do not publish a generic
`peak_memory_bytes` or `io_bytes` when the source semantics differ. Examples of
honest names are `max_resident_kib`, `job_peak_memory_bytes`,
`storage_read_bytes`, and `all_io_read_transfer_bytes`.

## Platform source review

### Portable Rust and `sysinfo`

[`available_parallelism`](https://doc.rust-lang.org/std/thread/fn.available_parallelism.html)
is a cheap, safe estimate of default parallel capacity, not a topology or
current-load API. Its own documentation records Windows Job Object and affinity
overcounting, Linux affinity/cgroup failure modes, VM overcommit, and the cost
of cgroup-v1 mount scans. Persist the value with approximate quality and the
collector version, never as “logical CPU count.”

[`sysinfo` 0.39](https://docs.rs/sysinfo/latest/sysinfo/struct.ProcessRefreshKind.html)
offers safe, targeted CPU, memory, and disk refreshes. It should be tested with
`System::new()` and narrow refresh kinds, not `System::new_all()` or all-process
enumeration. On Linux, “everything” can traverse every task; on Windows,
`Process::disk_usage` represents all I/O rather than Unix storage-layer I/O.
CPU percentage also needs retained prior state and a sampling interval, while
`accumulated_cpu_time` is a cumulative CPU-millisecond counter.

`sysinfo` is currently best treated as:

- a candidate safe wrapper for claim-time system CPU/memory facts;
- an oracle for comparing a lower-level attempt collector in the spike;
- a fallback only if sampling error and missed short attempts are measured and
  accepted explicitly.

It must never refresh user, cwd, executable path, command line, environment,
network interfaces, motherboard, or product identity for this feature.

### Windows

`command-group` already creates a Job Object for each spawned contained
process. Windows Job Objects include child processes by default and preserve
accounting for terminated members. Microsoft documents aggregate user/kernel
time and process counts in
[`JOBOBJECT_BASIC_ACCOUNTING_INFORMATION`](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_accounting_information),
all-process I/O counters in
[`JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION`](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_and_io_accounting_information),
and peak job memory in
[`JOBOBJECT_EXTENDED_LIMIT_INFORMATION`](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information).

This is the strongest current process-tree source, but `command-group` 5.0.1
keeps the Job handle private and exposes no resource snapshot. Direct Win32
calls from `crfty-engine` would require first-party `unsafe`, conflicting with
ADR-005. The preferred experiment is therefore a narrow safe resource snapshot
added to a reviewed dependency/fork (possibly upstreamed), not scattered
`windows-sys` calls in the engine. The snapshot must occur before the Job handle
closes and must not expose the handle or member PIDs.

Nested Job limits need separate treatment. Parent limits influence descendants,
and CPU-rate quotas are relative through the hierarchy. Querying the child Job
created by `command-group` may not reveal the full effective outer constraint.
`available_parallelism` likewise documents that it may overcount Job-limited
capacity. Record “not observable” rather than pretending the child limit is the
effective limit.

Whole-system CPU deltas are feasible through
[`GetSystemTimes`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getsystemtimes),
but systems with more than 64 processors require processor-group care. System
memory is available through standard memory APIs or a safe wrapper. These facts
remain experimental until a same-stage estimator experiment shows value.

### Linux

The terminal CPU source is
[`wait4`](https://www.man7.org/linux/man-pages/man2/wait4.2.html) plus
[`rusage`](https://man7.org/linux/man-pages/man2/getrusage.2.html). Linux reports
user/system CPU, maximum resident set in KiB, faults, block-operation counts,
and context switches. Some `rusage` members are unmaintained zeros, and
`ru_maxrss` for `RUSAGE_CHILDREN` is the largest child rather than a process-tree
peak. The collector must expose only documented maintained fields and identify
its scope.

`command-group` 5.0.1 currently calls `waitpid` and discards resource usage. A
reviewed dependency change could use `wait4` and return a safe terminal
snapshot. This is preferable to polling. A Linux-only experiment used
`waitid(..., WNOWAIT)` to leave exited direct children waitable before reading
`/proc/<pid>/io` and reaping them. Although the
[`waitid` contract](https://man7.org/linux/man-pages/man2/waitpid.2.html) supports
leaving the child waitable, the WSL fixture returned `EACCES` for the I/O file
on successful, killed, and fast-exiting zombies. Terminal `/proc` I/O is
therefore not a current recommendation; native/restricted Linux fixtures may
explain portability, but cannot turn this failure into a required collector.

[`/proc/<pid>/io`](https://www.kernel.org/doc/html/latest/filesystems/proc.html)
distinguishes characters passed through reads/writes from storage-layer bytes,
has filesystem caveats, can account canceled writes, is permission-gated, and
can tear on 32-bit systems. Persist only selected numeric counters. Never read
or retain `cmdline`, `environ`, `cwd`, `exe`, UID/GID, or process names.

For limits and pressure,
[`cgroup v2`](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html)
provides `cpu.max`, `cpu.stat`, effective cpusets, memory limits/peaks, per-device
I/O counters, and pressure files. Limits are hierarchical: a leaf default of
`max` does not prove the workload is unconstrained by an ancestor. Resolving the
current cgroup may require parsing mount and membership paths. Those paths are
collector-internal routing data and must be discarded immediately, never
journaled or exported. A shared cgroup's usage is not job usage.

[`PSI`](https://www.kernel.org/doc/html/latest/accounting/psi.html) reports time
that tasks are stalled for CPU, memory, or I/O, system-wide or per cgroup. It is
a contention signal, not an attribution signal. An encoder that deliberately
keeps more work runnable than CPUs can raise CPU pressure itself. Evaluate raw
counter deltas as an eligibility/weighting signal before trying a derived
“external contention” feature.

Creating a dedicated cgroup per attempt could improve attribution but would add
a new containment mechanism, privilege/delegation requirements, cleanup, and
interaction with the existing process group. It is out of the initial spike
unless simpler sources fail a required consumer.

### Encoder and accelerator relevance

Exact encoder revision is essential. SVT-AV1's
[`2.3.0` changelog](https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/CHANGELOG.md)
changed `--lp` from a logical-processor target to a level of parallelism; even
`--lp 1` can create multiple threads. A numeric setting without the encoder
revision is not a stable execution identity.

FFmpeg's [`-hwaccels` documentation](https://ffmpeg.org/ffmpeg.html) explicitly
separates build support from actual runtime availability. Therefore device
inventory or advertised support cannot replace the actual decoder recorded on
each attempt. The initial useful accelerator facts are the chosen decoder, its
role (decode, not AV1 encode), and fallback outcome.

Vendor telemetry such as NVML exposes useful utilization, temperature, clocks,
and power, but also serial numbers, PCI IDs, process IDs, and other prohibited
identity in the same API surface. It is not justified without measured
predictive benefit, a strict allowlist wrapper, equivalent AMD/Intel behavior,
and a dependency/unsafe review.

### Power and throttling

Linux exposes some Intel thermal throttle counters and power/energy zones, but
availability depends on CPU vendor, kernel drivers, sysfs permissions, and
virtualization. RAPL energy is package- or zone-wide, not inherently attributable
to one encode. Current frequency can differ from requested policy because of
hardware coordination, thermal limits, and power limits. See the kernel's
[`powercap`](https://www.kernel.org/doc/html/latest/power/powercap/powercap.html),
[`thermal throttle`](https://www.kernel.org/doc/html/latest/admin-guide/thermal/intel_thermal_throttle.html),
and [`CPU frequency`](https://www.kernel.org/doc/html/latest/admin-guide/pm/cpufreq.html)
documentation.

Windows power policy similarly does not directly provide per-attempt thermal or
power attribution. Until these measurements beat simpler execution identity
and CPU-time normalization, initial production should omit them rather than
ship a mostly-null, vendor-specific collector.

## Preliminary Windows fixture

This fixture used a throwaway PowerShell/C# probe to call the documented Win32
APIs directly. It created an unnamed Job Object, assigned a synthetic child,
waited for the child to exit, and only then queried accounting. No host, user,
path, command-line, process ID, device, or Job handle value was printed or
retained.

Environment:

- Windows build 10.0.26100, x64;
- 12 processors reported to the process environment;
- the probe process was not already inside an outer Job Object, so nested and
  constrained-parent behavior remains untested.

The direct-child workload touched a 64 MiB allocation, performed CPU work, and
wrote/read/deleted a 4 MiB temporary file. Across five runs:

- accounting remained queryable after `WaitForExit`, with `ActiveProcesses = 0`;
- `TotalProcesses = 1`, user/kernel CPU and page-fault counts were nonzero;
- read transfer was about 4.49 MB and write transfer about 4.19 MB, illustrating
  that Job I/O includes process/runtime activity in addition to the explicit
  payload;
- peak process/job memory was 192.5–193.1 MB, not the allocation size, because
  the counter measures the complete process rather than the test payload;
- `TotalTerminatedProcesses = 0`. This field means processes terminated because
  of a Job limit violation, not ordinary exited processes, and must not be
  modeled as an exit count.

A second workload kept the parent allocation alive while spawning a synthetic
descendant. The Job reported three associated processes and zero active after
wait. Aggregate peak Job memory was 307,531,776 bytes while the largest-process
peak was 193,609,728 bytes; aggregate CPU, faults, and I/O also increased. This
proves that Job-level evidence captures materially different scope than leader
PID polling. The fixture intentionally did not enumerate or retain which helper
processes made up that count.

The C# helper measured native calls internally for 10,000 iterations, avoiding
PowerShell loop overhead. Five direct-child trials produced:

| Operation | Minimum | Median | Maximum |
| --- | ---: | ---: | ---: |
| Query Job basic + I/O accounting | 0.600 µs | 0.616 µs | 0.618 µs |
| `GetSystemTimes` | 4.067 µs | 4.280 µs | 5.327 µs |
| `GlobalMemoryStatusEx` | 1.322 µs | 1.376 µs | 1.385 µs |

This supports terminal-boundary collection rather than polling, but it does not
yet prove that `command-group` can expose the snapshot without changing handle,
wait, cancellation, or completion-port behavior. It also does not test an outer
Job, CPU-rate limit, affinity restriction, cancellation, or very fast child.
The throwaway source was removed after recording the fixture.

## Preliminary Linux/WSL fixture

This is a capability and read-cost fixture, not the required cross-platform
collector benchmark.

Environment:

- Linux 6.18 WSL2 class, x86_64;
- 12 available processors reported by the environment;
- cgroup v2 mounted;
- system CPU, memory, and I/O PSI files readable;
- `/proc/self/stat`, `/proc/self/status`, and `/proc/self/io` readable;
- cgroup root exposed usage/pressure but no leaf `cpu.max` or `memory.max`,
  demonstrating that a collector cannot assume limit files at the mount root;
- no cpufreq policy or powercap zone exposed; thermal cooling devices alone did
  not provide a portable temperature/throttle measurement.

A throwaway optimized Rust program performed 10,000 cached open-and-read
operations per trial for five trials. It did no parsing or serialization and
printed no source contents.

| Operation | Minimum | Median | Maximum |
| --- | ---: | ---: | ---: |
| `std::thread::available_parallelism()` | 22.605 µs | 22.913 µs | 26.284 µs |
| Read `/proc/self/{stat,status,io}` bundle | 36.962 µs | 37.107 µs | 37.683 µs |
| Read `/proc/stat`, `/proc/meminfo`, and three PSI files | 63.950 µs | 65.247 µs | 65.959 µs |

These numbers only show that narrow cached reads are plausible at startup,
claim, or terminal boundaries on this host. They do not establish production
overhead, parsing cost, Windows cost, cold-cache behavior, correctness under
process exit, or an acceptable polling interval. The throwaway source and
binary were removed after measurement.

### Linux terminal/reap fixture

A second throwaway native fixture tested the lifecycle that a modified
`command-group` would actually own. For each direct child, the parent called
`waitid(P_PID, ..., WEXITED | WNOWAIT)`, attempted allowlisted `/proc` reads,
then called `wait4` and checked that `/proc/<pid>` disappeared after reaping.
Cases covered normal success, a child that waited for its own descendant, an
immediate nonzero exit, and forced `SIGKILL`.

Results on this WSL2 kernel:

- `wait4` returned user/system CPU, maximum RSS, faults, block operations, and
  context switches in all four cases, including immediate exit and `SIGKILL`.
- The successful direct child reported about 400 ms user CPU, 87 ms system CPU,
  67,340 KiB maximum RSS, and 8,192 output block operations after touching
  64 MiB and writing 4 MiB.
- When that child waited for a descendant that touched 32 MiB, used CPU, and
  wrote another 2 MiB, the returned totals rose to about 807 ms CPU, 99,996 KiB
  maximum RSS, and 12,288 output block operations. This is evidence that
  waited-descendant usage can reach the leader's terminal `rusage` on this
  kernel; the portable contract still needs native Linux repetition and must
  not claim a simultaneous process-tree RSS peak from one observation.
- `/proc/<pid>/io` returned permission denied after `waitid(WNOWAIT)` in every
  case. `/proc/<pid>/status` remained readable for the zombie but `VmHWM` was
  zero, so it did not recover terminal peak memory. The process directory
  disappeared immediately after `wait4` reaped it.

This materially narrows the Linux recommendation: retain `wait4` CPU/max-RSS/
fault/block/context-switch fields for further validation; do not depend on a
terminal `/proc` byte-I/O or `VmHWM` collector. Obtaining byte I/O would require
active polling, task accounting, or a dedicated cgroup, each of which has more
cost and scope complexity and needs a consumer before further work.

## Required spike matrix

### Environments

- native Windows on ordinary hardware;
- Windows while the application is already inside an outer Job Object, with a
  CPU-rate or affinity restriction where the test environment permits it;
- native Linux with cgroup v2;
- Linux in a delegated/restricted container and a cgroup with explicit CPU and
  memory limits;
- WSL2;
- restricted `/proc`/permission case;
- at least one virtualized or overcommitted environment.

Every fixture records capabilities and typed absences. Environment-specific
paths, machine names, account names, IDs, and device addresses are excluded.

### Workloads

- short successful direct child, proving fast exits are not missed;
- CPU-bound child with known parallelism;
- memory-ramp child with a known retained peak;
- cached read, storage read, write, and truncate/canceled-write cases;
- child that spawns a descendant, proving the actual accounting scope;
- nonzero exit, cancellation, forced termination, and start failure;
- concurrent unrelated CPU and I/O load;
- real ab-av1 search with multiple VMAF targets;
- hardware-search failure followed by software retry;
- hardware-encode failure followed by software retry;
- successful encode, remux, not-worthwhile, stopped, and crash-recovered run.

### Implementations to compare

1. No collector control.
2. Targeted `sysinfo` snapshots with only CPU/memory/I/O refreshes.
3. A safe `command-group`/process-wrapper terminal snapshot:
   Windows Job Object accounting and Linux `wait4` resource usage.
4. Linux `waitid(WNOWAIT)` plus allowlisted `/proc/<pid>/io` only as a negative
   portability fixture; current WSL evidence rejects it as a dependable
   terminal source.
5. Claim/terminal system snapshots, with PSI separately feature-gated on Linux.

No candidate may survive merely because it is easy to collect. It must satisfy
a consumer and improve correctness or held-out prediction enough to justify its
cost and privacy surface.

### Measurement method

- Alternate collector-disabled and collector-enabled runs to reduce drift.
- Report median, p90/p95, and worst observed collector latency separately from
  workload wall-time change.
- Measure allocations and bytes read where tooling allows.
- Include warm and cold startup, short-process miss rate, and terminal race
  rate, not just long encodes.
- Compare every counter with a workload-controlled expectation and document
  semantic mismatches rather than converting them away.
- Run cancellation and panic paths; telemetry failure must never abort media
  work or suppress terminal outcome.
- Apply the stage budgets selected by #92 only after that issue freezes them.

### Privacy tests

Seed synthetic host, user, path, command-line, cgroup-path, serial, UUID, bus,
PID, and environment canaries into the test environment. Assert that none can
enter the pathless serialized fixture. The production shape should be built
from typed allowlisted scalars/enums, so the test verifies an invariant rather
than depending on a blacklist scrubber.

Collector routing may temporarily observe a PID, Job handle, cgroup path, or
mount root. These are ephemeral implementation details. They must not cross the
collector boundary, enter a diagnostic message, or be used to construct a
hardware/environment fingerprint.

## Estimator experiments

Use content identity to keep observations of the same media out of both train
and held-out sets. Also report tool-version and environment-held-out results so
memorizing one installation cannot look like portability.

Compare incrementally:

- **B0**: current operation + codec + resolution median-rate baseline.
- **B1**: exact execution settings and tool-revision eligibility/cohorting.
- **B2**: B1 plus available parallelism and observed resource caps.
- **B3**: B2 with failed fallback attempts separated and contended samples
  filtered or downweighted.
- **B4**: claim-time load adjustment, evaluated only at claim/live stages.
- **B5**: coarse topology/capability or calibration only if prior steps leave a
  material cross-machine error and privacy review permits the candidate.

Report median absolute percentage error, p90 absolute percentage error, signed
bias, and coverage. Segment results by Analyze/Convert, resolution, preset,
decoder, machine class, and tool revision. The material-improvement threshold
and minimum coverage belong to #92; this research must not invent them after
seeing results.

Keep raw source counters and version derived features. For example, a proposed
external-busy derivation may compare whole-system busy CPU time with job CPU
time, but only when both scopes and capacity denominator match. Clamp-underflow
or scope mismatch must produce an unavailable derivation, not zero contention.

## Recommended dependency and integration direction

The first spike should target the process boundary already shared by V3 and the
pinned ab-av1 fork:

1. Define a tiny safe terminal-resource snapshot in the process containment
   layer, with source-specific optional fields and no identifiers.
2. Prototype Windows Job Object accounting and Linux `wait4` behind that
   interface. Because ADR-005 forbids first-party unsafe and `command-group`
   owns the necessary Windows handle/reap logic, prefer a reviewed dependency
   extension over engine-local OS calls.
3. Thread the snapshot through the pinned ab-av1 library's `ManagedChunkStream`
   terminal report so every FFmpeg search/encode attempt can retain it. The
   global running-child list currently owns the only `AsyncGroupChild` handles.
4. Add an explicit attempt ledger before attaching resource facts. Keep current
   phase spans as the run envelope.
5. Evaluate targeted `sysinfo` only for claim-time system context and as a
   cross-check. Do not add it solely to poll child PIDs.
6. Keep PSI, topology, virtualization, and hardware facts feature-gated within
   the spike. Treat Linux terminal `/proc` I/O as rejected unless a required
   consumer and materially different native evidence justify reopening it.
7. Delete spike-only implementations that are rejected; #99 implements only
   the selected collectors.

This direction is a research recommendation, not permission to fork a
dependency or change the durable schema. It must be validated by the matrix
above and reconciled with #93's final observation contract.

## Unresolved questions

- Which estimates and statistics actually consume each fact, and what are their
  stage budgets (#92)?
- Does exact execution/tool cohorting outperform the current baseline before
  any environment telemetry is added?
- Does `wait4` account enough of the real FFmpeg workload on Linux, including
  any descendants used by supported tools?
- Does native Linux reproduce the WSL finding that an exited, unreaped child
  denies `/proc/<pid>/io`, and do any supported environments differ enough to
  justify more than a recorded typed absence?
- Can `command-group` expose Windows Job accounting without changing its
  containment and cancellation guarantees?
- Which Windows outer-Job constraints can be observed safely and truthfully?
- Are peak memory and I/O useful to an actual user-facing consumer, or merely
  interesting operational telemetry?
- Can claim-time load improve held-out queue estimates without amplifying noise
  from the immediately preceding serial encode?
- Is a coarse CPU capability/performance class worth its linkability, or is
  per-environment historical calibration enough?
- Should failed/stopped attempt resource evidence be visible in History,
  operational activity, or estimator-only evidence (#93)?
- What disclosure does the pathless bundle need for combinations of otherwise
  non-identifying technical facts (#97)?

## Exit criteria for issue #94

The research is ready to feed #96/#99 only when:

- Windows, native Linux, WSL, restricted, constrained, and virtualized fixtures
  are committed with only synthetic/pathless facts;
- collector-disabled/enabled overhead and miss/race rates are measured;
- process-tree scope and units are proven rather than inferred;
- each candidate has a consumer, stage, provenance, quality, null behavior,
  privacy classification, and evidence-backed disposition;
- estimator comparisons report held-out error, bias, and coverage;
- prohibited identifiers are structurally unable to enter portable output;
- rejected dependencies and spike code are removed;
- #93 incorporates the surviving evidence semantics before #96 selects a
  logical model and #99 implements collectors.
