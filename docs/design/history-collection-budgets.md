# History collection budgets

Status: working design note; candidate caps are proposals with recorded derivations, and no cap is authoritative until `docs/PLAN.md` records it as decided

## Purpose and boundary

Collection budgets are policy caps on what History collection may cost each pipeline stage, fixed before field and collector selection so that a candidate collector earns its place inside a stated ceiling instead of negotiating one after the fact. This note derives candidate caps from the measured evidence in `docs/design/history-content-evidence-research.md` and `docs/design/history-environment-runtime-research.md` and records each derivation.

It selects no fields and no collectors, defines no observation shapes, and measures nothing itself. A cap is a product decision about acceptable cost. The measurements below inform where a cap can sit; they do not turn the cap into a measurement.

## Evidence labels

- **measured** and **read from source**: cited from the recorded runs and source readings in the two research notes.
- **derived**: computed from cited values, with the derivation stated.
- **policy**: a proposed product ceiling, chosen rather than measured.

## What a budget binds

`docs/PLAN.md` records the decided form: a collector attached to a search is capped as a share of that search, while Basic Scan, startup, and browsing carry absolute targets. Four clarifications are proposed on top of it:

- A cap binds the aggregate of all collectors at its surface, not each collector separately. The user experiences the sum, and per-collector caps invite additive creep.
- A cap binds the increment attributable to History collection, not work the stage already performs. Basic Scan's probe and identity sampling and startup's tool discovery are existing costs outside these budgets.
- A share cap is evaluated at selection time as an aggregate over the representative workload, never enforced per file at runtime. Per-file share varies inversely with attempt count and search length: the window pass measured 1.37 to 3.92 percent across successful searches and 10.68 percent against a search that failed after one attempt, with nothing wrong.
- A cap binds History collection reads, not operational streams the pipeline already consumes. The adapter's typed updates and the remux `-progress` output sit outside every cap here, including the polling prohibition.

## Candidate caps

| Surface | Form | Candidate cap |
| --- | --- | --- |
| Quality search | share of the informed search | 5 percent, aggregate |
| Lifecycle boundaries (job claim, attempt start, attempt reap, job terminal) | absolute per boundary | 1 ms, boundary reads only, no periodic sampling |
| Basic Scan | absolute per file | no added process, no added file read, 1 ms added CPU |
| Startup | absolute on the critical path | 10 ms aggregate, no process launch |
| Browsing | absolute | first History paint under 100 ms at 50,000 records (decided), extended to filters, sorts, and the Statistics recompute |

### Quality search: 5 percent, aggregate

**Measured.** Across the twenty collected fixtures the window filter pass cost 1.37 to 3.92 percent of every successful search it would inform, spanning animation, film, live action, motion, grain, and three frame sizes. The whole-file packet pass cost 0.03 to 0.67 percent on moderate-bitrate fixtures and 15.33 percent on the 86 Mbps grain-heavy fixture. The spatial pass produced no usable value on any of the twenty fixtures.

**Derived.** The share form is stable for decode-bound passes because they and the search scale with the same drivers, sample count and pixel rate. It is not stable for whole-file demux, whose cost scales with container bytes and storage speed, so the packet pass as designed blows through any search-relative ceiling on high-bitrate sources. Excluding that byte-bound outlier, the worst measured packet-plus-window total stays 3.94 percent, on the shortest successful search.

**Policy.** Five percent aggregate. It admits the window pass across every measured content class with margin for host variance, and it refuses the spatial pass as measured alongside it. It also refuses whole-file demux on high-bitrate sources: a surviving packet collector must bound its reads instead of scaling with container bytes. Applying that selection pressure is what the cap is for.

**Read from source.** Retaining the sample and attempt facts ab-av1 already streams to the adapter adds no process and no pass, so its stage cost is negligible by construction; its cost is storage, covered below.

### Lifecycle boundaries: 1 ms, no sampling

**Measured.** A Windows Job accounting query took under a microsecond, Linux terminal usage arrives with the reap the supervisor already performs, and the claim-time system snapshot bundle read in about 65 µs.

**Policy.** One millisecond per boundary, three orders of magnitude above the cited reads, so it never binds a selected boundary collector. Its content is the shape constraint: collection happens at the discrete lifecycle boundaries named above, never as a polling loop. Polling is where cost, missed short attempts, and misattribution live, and admitting a sampled collector means reopening this cap, not stretching it. The measured reads were warm-cache; the two orders of headroom under the cap are what absorb cold-cache behaviour, which no fixture has established.

### Basic Scan: nothing the scan is not already doing

The content-feature pass runs with Analyze and never during Basic Scan, so the surviving scan-stage candidates are facts parsed from ffprobe output the scan already fetched and from the identity sampling it already performs.

**Policy.** No added process, no added file read, and at most 1 ms of added CPU per file, held by a tripwire: a Basic Scan over the representative workload with collection enabled regresses its wall time by less than 2 percent against collection disabled.

### Startup: 10 ms, no process launch

**Measured.** The constant-time environment reads are microseconds: the parallelism estimate about 23 µs, the limit and pressure file bundle about 65 µs.

**Policy.** Ten milliseconds aggregate on the startup critical path, and no process launch on it. The one expensive candidate is the synthetic compatibility probe that runs the selected FFmpeg against a lavfi source to confirm executable identity and effective settings. The probe is unmeasured and runs after first paint or at first claim, and until it has run its facts are typed absences like any other. First paint never waits for collection.

**Policy.** Wherever the probe runs, its budget home is the search-share aggregate, amortized once per session and configuration, because a 1 ms boundary cap cannot host a process launch and startup excludes one outright. Its cost stays unmeasured today, so a measurement precedes any collector decision that admits it.

### Browsing: one tripwire for the whole surface

First History paint under 100 ms at 50,000 records is decided in `docs/PLAN.md` as a regression tripwire. The proposal here is that the same tripwire covers filter toggles, sort changes, and the Statistics recompute at the same corpus size, because a fast first paint into a view whose interactions stall does not keep the guardrail's promise. For an interaction the measured quantity is input to settled frame over the same corpus, so the guardrail keeps one number and one fixture set.

## Storage is a cost these caps do not bind

The caps above bind stage time. Retained sample and attempt evidence costs bytes instead: the measured corpus kept roughly one in six of the attempt observations its searches paid for, and retaining the rest multiplies durable rows per search severalfold. No byte cap is derived here, because journal growth is bounded by compaction and no record-size measurement against the representative workload exists yet. The freeze should either record a per-run size tripwire or record that storage is deliberately unbudgeted, so that silence does not read as a decision. Field selection is the natural revisit trigger: the decision that fixes the observation shape is the same one that makes a bytes-per-search measurement possible, and no collector ships ahead of it.

## The workload every tripwire assumes

Each tripwire above measures against a representative workload, and no such fixture corpus exists yet. Defining it belongs to the measurement work that consumes these caps, and no tripwire is acceptance-testable before that corpus lands.

## Open decisions

- The aggregate reading of the share cap, and whether 5 percent is the right ceiling.
- Whether the polling prohibition at lifecycle boundaries is absolute for the release or admits a measured exception.
- The startup number, and whether the compatibility probe runs after first paint or at first claim.
- Whether the browsing tripwire covers interactions and Statistics or first paint alone.
- A storage tripwire, or an explicit decision not to set one.

## Distillation

Frozen caps are recorded as decided in `docs/PLAN.md`, derivations worth keeping move into the contract documents that replace the research notes, and this note is deleted when the freeze lands.
