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

`docs/PLAN.md` records the decided form: a collector attached to a search is capped as a share of that search, while Basic Scan, startup, and browsing carry absolute targets. Two clarifications are proposed on top of it:

- A cap binds the aggregate of all collectors at its surface, not each collector separately. The user experiences the sum, and per-collector caps invite additive creep.
- A cap binds the increment attributable to History collection, not work the stage already performs. Basic Scan's probe and identity sampling and startup's tool discovery are existing costs outside these budgets.

## Candidate caps

| Surface | Form | Candidate cap |
| --- | --- | --- |
| Quality search | share of the informed search | 5 percent, aggregate |
| Attempt boundaries (claim and terminal, for search, encode, and remux) | absolute per boundary | 1 ms, boundary reads only, no periodic sampling |
| Basic Scan | absolute per file | no added process, no added file read, 1 ms added CPU |
| Startup | absolute on the critical path | 10 ms aggregate, no process launch |
| Browsing | absolute | first History paint under 100 ms at 50,000 records (decided), extended to filters, sorts, and the Statistics recompute |

### Quality search: 5 percent, aggregate

**Measured.** On the four collected fixtures the whole-file packet pass cost 0.03 to 0.07 percent of the search it would inform, and the window filter pass cost 2.29 to 3.87 percent. The spatial pass cost 1.28 to 5.80 percent while producing no usable value.

**Derived.** The share form is stable because the candidate passes and the search scale with the same drivers, sample count and pixel rate, where absolute seconds do not transfer across content lengths. The worst measured admissible set, packet plus window on the shortest search, totals 3.94 percent.

**Policy.** Five percent aggregate. It admits the packet and window passes together with margin for host variance, and it refuses the spatial pass as measured alongside them: a spatial collector must displace the window pass or get cheaper. Applying that selection pressure is what the cap is for.

**Read from source.** Retaining the sample and attempt facts ab-av1 already streams to the adapter adds no process and no pass, so its stage cost is negligible by construction; its cost is storage, covered below.

### Attempt boundaries: 1 ms, no sampling

**Measured.** A Windows Job accounting query took under a microsecond, Linux terminal usage arrives with the reap the supervisor already performs, and the claim-time system snapshot bundle read in about 65 µs.

**Policy.** One millisecond per boundary, three orders of magnitude above the cited reads, so it never binds a selected boundary collector. Its content is the shape constraint: collection happens at claim and terminal boundaries, never as a polling loop. Polling is where cost, missed short attempts, and misattribution live, and admitting a sampled collector means reopening this cap, not stretching it.

### Basic Scan: nothing the scan is not already doing

The content-feature pass runs with Analyze and never during Basic Scan, so the surviving scan-stage candidates are facts parsed from ffprobe output the scan already fetched and from the identity sampling it already performs.

**Policy.** No added process, no added file read, and at most 1 ms of added CPU per file, held by a tripwire: a Basic Scan over the representative workload with collection enabled regresses its wall time by less than 2 percent against collection disabled.

### Startup: 10 ms, no process launch

**Measured.** The constant-time environment reads are microseconds: the parallelism estimate about 23 µs, the limit and pressure file bundle about 65 µs.

**Policy.** Ten milliseconds aggregate on the startup critical path, and no process launch on it. The one expensive candidate is the synthetic compatibility probe that runs the selected FFmpeg against a lavfi source to confirm executable identity and effective settings. The probe is unmeasured and runs after first paint or at first claim, and until it has run its facts are typed absences like any other. First paint never waits for collection.

### Browsing: one tripwire for the whole surface

First History paint under 100 ms at 50,000 records is decided in `docs/PLAN.md` as a regression tripwire. The proposal here is that the same tripwire covers filter toggles, sort changes, and the Statistics recompute at the same corpus size, because a fast first paint into a view whose interactions stall does not keep the guardrail's promise.

## Storage is a cost these caps do not bind

The caps above bind stage time. Retained sample and attempt evidence costs bytes instead: the measured run kept roughly one fifth of the attempt observations its searches paid for, and retaining the rest multiplies durable rows per search severalfold. No byte cap is derived here, because journal growth is bounded by compaction and no record-size measurement against the representative workload exists yet. The freeze should either record a per-run size tripwire or record that storage is deliberately unbudgeted, so that silence does not read as a decision.

## Open decisions

- The aggregate reading of the share cap, and whether 5 percent is the right ceiling.
- Whether the polling prohibition at attempt boundaries is absolute for the release or admits a measured exception.
- The startup number, and whether the compatibility probe runs after first paint or at first claim.
- Whether the browsing tripwire covers interactions and Statistics or first paint alone.
- A storage tripwire, or an explicit decision not to set one.

## Distillation

Frozen caps are recorded as decided in `docs/PLAN.md`, derivations worth keeping move into the contract documents that replace the research notes, and this note is deleted when the freeze lands.
