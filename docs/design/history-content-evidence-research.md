# V3 History: sample-attempt and content-complexity evidence research

Status: living research note; not an accepted design or implementation specification

Owning issue: [#95](https://github.com/Loufe/AB-AV1-GUI/issues/95)

Related issues: [#57](https://github.com/Loufe/AB-AV1-GUI/issues/57), [#92](https://github.com/Loufe/AB-AV1-GUI/issues/92), [#93](https://github.com/Loufe/AB-AV1-GUI/issues/93), [#94](https://github.com/Loufe/AB-AV1-GUI/issues/94), [#96](https://github.com/Loufe/AB-AV1-GUI/issues/96), [#100](https://github.com/Loufe/AB-AV1-GUI/issues/100)

## Purpose and boundary

This note records what a first measurement pass established about two candidate sources of History evidence: the per-sample and per-attempt facts that ab-av1 already produces during a quality search and then discards, and the content-complexity features that cheap FFmpeg collectors can compute before any encode begins.

It does not select fields, choose between typed content columns and versioned feature payloads, or authorize the collectors in issue #100. It settles nothing about the material-improvement threshold, which belongs to #92, or about the evidence-quality contract, which belongs to #93.

The measurement harness is standalone research tooling that lives outside this repository and is never committed to it, so its method appears here as prose and its output appears here as tables. The section below records where it lives, because this note is the only durable pointer to it.

## Evidence labels

- **measured**: a value produced by the harness run recorded in this note.
- **measured, single environment**: measured once, on one host and one toolchain, with no repetition and no second machine.
- **derived**: computed from measured values, with the derivation stated.
- **read from source**: established by reading pinned dependency or first-party code rather than by measurement.
- **unverified**: carried from earlier investigation or from reasoning, not reproduced by this run, and not admissible in a contract.

## What the pinned adapter already sees and discards

**Read from source.** V3 consumes the ab-av1 fork in process (ADR-003, embed a pinned ab-av1 adapter), so `command::crf_search::run` hands the adapter typed updates rather than text. In `crfty-engine/src/ab_av1/operation.rs` the search loop consumes `Update::Status` into live telemetry, converts `Update::Done` into the one `SearchOutcome` that becomes durable, converts the `NoGoodCrf` error into a failure carrying its final outcome, and matches every remaining update with an empty arm.

Those discarded updates are `SampleResult`, which carries per-sample raw and encoded byte sizes, VMAF and XPSNR scores, sample encode wall time, sample duration, and a cache flag, and `RunResult`, which carries the per-attempt summary for every attempt except the successful final one.

**Read from source.** Telemetry is explicitly not a durable source in V3: phase spans and outcomes reach the reducer through the lossless terminal command, and `Update::Status` feeds only the live view. Retaining attempt evidence therefore means adding durable observations, not re-reading telemetry.

**Read from source.** Retaining these facts is an adapter change inside the existing patch surface. It widens no fork patch and adds no process, because the updates already arrive on the stream the adapter is reading.

## Where the harness lives

The harness is at `~/crfty-spike95-harness/` in WSL. It is local, standalone, and deliberately outside the repository: it is Python, the rewrite forbids committing Python, and it is never imported by the application. Nothing in it is a build input, and nothing in it should be salvaged into the repository. Its `README.md` covers toolchain setup and how to rerun the pass.

It contains:

- `README.md`: setup, toolchain install, rerun instructions, and the analysis contract the results must be evaluated under.
- `collect.py`: the per-fixture collector. One run performs the baseline ffprobe, the demux-only packet pass, the window filter pass over ab-av1's deterministic sample geometry, the spatial pass at a fixed analysis scale, and the ground-truth quality search, timing each independently and writing one record to `results/`.
- `prep.sh`: cuts downloaded sources into the labeled fixtures, including plain segments, x264 re-encodes of pristine y4m masters, controlled-grain variants, and one deliberately low-headroom recompressed source.
- `download.sh`: re-fetches every source. Open content only, Blender open movies through archive.org and Xiph derf masters.
- `run_all.sh`: preparation plus the collection loop, run sequentially so the timings stay clean.
- `results/`: one JSON record per fixture, self-describing, with toolchain versions and the ab-av1 arguments recorded inside each record.
- `pipeline.log`: the run record for the pass reported in this note.

The working area is `~/spike95/` in WSL, holding the pinned toolchain under `bin/`, the cut `fixtures/`, and `scratch/`. All of it is disposable and re-creatable by rerunning `download.sh` and `prep.sh`; only the harness directory itself carries anything worth keeping.

The harness README also fixes the analysis contract, and it is the reason this pass cannot be scored. Held-out comparison must group by source film, because segments of one film share style and a naive split leaks between train and test. Prediction targets are best CRF, predicted encode percent, and search wall time. Baseline features are codec, width, height, duration, frame rate, bitrate, and bits per pixel per frame.

## The measured run

**Measured, single environment.** One collection pass ran on a WSL2 host against fixtures cut from open content (Blender open movies and Xiph derf masters). Timings from this environment are meaningful as ratios between passes on the same host, not as absolute costs on user hardware.

| Setting | Value |
| --- | --- |
| Encoder preset | 6 |
| Minimum VMAF | 95.0 |
| ab-av1 sample cache | disabled, so no attempt replayed a cached timing |
| ab-av1 | 0.11.5, invoked as a CLI with `--stdout-format json` |
| FFmpeg/ffprobe | master build `N-126175-g0056dd32fd-20260816` |
| Analysis scale for the spatial pass | 480x270 |
| Sample window duration | 20 s |

**Measured.** Every fixture in this run was about 183 s long, which under ab-av1's sampling geometry yields exactly one 20 s window per search, so each fixture contributes one window rather than a window series.

The ground-truth toolchain is not V3's toolchain, and the gap matters when reading the numbers below. V3 pins an ab-av1 0.11.4 fork consumed in process, and `--stdout-format json` postdates that base; the harness used the CLI and its JSON output purely to capture attempts that the in-process adapter would receive as typed updates. FFmpeg was a master build rather than V3's vendored artifact.

### Fixtures

**Measured.** All four completed fixtures are segments of one animated film, three at 1080p and one at 2160p. The 1080p segments are the interesting group: they agree on every predictor the current estimator uses.

| Fixture | Category | Codec | Resolution | FPS | Duration (s) | Size (bytes) | Bitrate (kbps) | Bits per pixel per frame |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| bbb1080_s30 | animation 1080p60 | h264 | 1920x1080 | 60 | 182.7 | 75,518,749 | 3,307 | 0.02658 |
| bbb1080_s240 | animation 1080p60 | h264 | 1920x1080 | 60 | 183.3 | 95,100,559 | 4,151 | 0.03336 |
| bbb1080_s450 | animation 1080p60 | h264 | 1920x1080 | 60 | 181.2 | 91,074,971 | 4,021 | 0.03232 |
| bbb2160_s30 | animation 4K60 | h264 | 3840x2160 | 60 | 182.7 | 151,454,805 | 6,632 | 0.01333 |

## Search outcomes

**Measured.** Each search ran to a successful result.

| Fixture | Best CRF | Achieved VMAF | Predicted encode percent | Predicted output (bytes) | Attempts | Search wall time (s) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| bbb1080_s30 | 35.25 | 95.05 | 69.42 | 50,661,504 | 5 | 199.8 |
| bbb1080_s240 | 37.75 | 95.07 | 50.31 | 47,841,357 | 3 | 129.1 |
| bbb1080_s450 | 58.75 | 95.39 | 9.96 | 8,823,377 | 6 | 220.5 |
| bbb2160_s30 | 45.25 | 95.05 | 44.18 | 66,917,709 | 5 | 745.9 |

**Measured.** The three 1080p segments share codec, resolution, frame rate, and duration within one percent, and their bitrates fall inside a 25 percent band. The current estimator groups on codec and a resolution bucket, so it would treat these three files as one cohort and predict one number for all of them. Their actual outcomes span best CRF 35.25 to 58.75 and predicted encode percent 69.42 to 9.96, a factor of seven in predicted output size.

That spread is the headroom a content signal would have to capture. It exists independently of whether any feature measured below actually captures it.

## Attempt ladders

**Measured.** Nineteen attempts were produced across the four searches. Four of them, one per search, correspond to the `Done` update that V3 retains today. The other fifteen are the observations the adapter currently drops.

| Fixture | Attempt | CRF | VMAF | Predicted encode percent | Retained by V3 today |
| --- | ---: | ---: | ---: | ---: | --- |
| bbb1080_s30 | 1 | 37.50 | 94.46 | 60.9 | no |
| bbb1080_s30 | 2 | 18.00 | 97.86 | 203.0 | no |
| bbb1080_s30 | 3 | 34.50 | 95.25 | 73.0 | no |
| bbb1080_s30 | 4 | 35.50 | 94.98 | 68.2 | no |
| bbb1080_s30 | 5 | 35.25 | 95.05 | 69.4 | yes |
| bbb1080_s240 | 1 | 37.50 | 95.16 | 51.2 | no |
| bbb1080_s240 | 2 | 57.00 | 84.60 | 14.9 | no |
| bbb1080_s240 | 3 | 37.75 | 95.07 | 50.3 | yes |
| bbb1080_s450 | 1 | 37.50 | 98.54 | 31.0 | no |
| bbb1080_s450 | 2 | 57.00 | 96.06 | 11.4 | no |
| bbb1080_s450 | 3 | 70.00 | 77.43 | 3.1 | no |
| bbb1080_s450 | 4 | 57.75 | 95.80 | 10.7 | no |
| bbb1080_s450 | 5 | 58.25 | 95.63 | 10.5 | no |
| bbb1080_s450 | 6 | 58.75 | 95.39 | 10.0 | yes |
| bbb2160_s30 | 1 | 37.50 | 97.20 | 72.3 | no |
| bbb2160_s30 | 2 | 57.00 | 89.55 | 20.7 | no |
| bbb2160_s30 | 3 | 43.00 | 95.77 | 50.7 | no |
| bbb2160_s30 | 4 | 44.75 | 95.21 | 45.6 | no |
| bbb2160_s30 | 5 | 45.25 | 95.05 | 44.2 | yes |

**Measured.** Each discarded row is a paid-for CRF-to-VMAF observation on real user content at a known preset. The searches spent between 129 s and 746 s producing them, and V3 keeps roughly one fifth of what it bought.

**Measured.** Two ladders show the interpolation model overshooting badly enough to spend an attempt on a clearly wrong CRF. The bbb1080_s450 search jumped to CRF 70 and measured VMAF 77.43 against a target of 95, then walked back through 57.75, 58.25, and 58.75. The bbb1080_s30 search jumped down to CRF 18 and measured VMAF 97.86 at a predicted encode percent of 203, meaning a predicted output twice the size of its input. Retained attempt observations are exactly the data a better prior would be fitted on.

**Measured.** Log lines are a worse source than the structured stream for the same run. Parsing ab-av1's stderr recovered attempt boundaries for only 16 of the 19 attempts (4 of 5, 3 of 3, 6 of 6, and 3 of 5 by fixture) and recovered no per-sample score lines at all, while the structured output carried every attempt in all four runs. This is measured support for consuming the adapter's typed updates rather than adding a log parser.

## Collector cost

**Measured, single environment.** Three collector passes ran per fixture, each timed independently: a demux-only packet pass over the whole file, a filter pass over the sample window computing scene-change, entropy, bit-plane noise, and crop detection in one decode, and a spatial-information pass at a fixed 480x270 analysis scale over the same window.

| Fixture | Packet pass (s) | Window filter pass (s) | Spatial pass (s) | Search wall time (s) | Packet pass share of search | Window pass share of search |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| bbb1080_s30 | 0.098 | 4.818 | 7.326 | 199.8 | 0.05% | 2.41% |
| bbb1080_s240 | 0.094 | 4.999 | 7.489 | 129.1 | 0.07% | 3.87% |
| bbb1080_s450 | 0.109 | 5.048 | 7.702 | 220.5 | 0.05% | 2.29% |
| bbb2160_s30 | 0.201 | 18.792 | 9.544 | 745.9 | 0.03% | 2.52% |

**Measured.** The spatial pass produced no usable value on any fixture. All four runs completed and consumed between 7.3 s and 9.5 s, and in all four the summary block the collector expected was absent, so no spatial-information or temporal-information value was recorded. Whatever this pass cost, this run bought nothing with it, and the spatial slot in the feature set remains unmeasured rather than filled.

**Derived.** Cost scales with pixel count rather than with window length. Between the 1080p and 2160p cuts of identical content and identical frame count, the window filter pass rose from 4.818 s to 18.792 s, a factor of 3.90 for four times the pixels, while the packet pass rose by a factor of 2.05 and the search itself rose by a factor of 3.73.

**Derived.** Expressing collector cost as a share of the search it would inform is the stable form, because both scale with sample count. On these fixtures the whole-file packet pass cost under a tenth of a percent of the search, and the window filter pass cost between 2.3 and 3.9 percent. Absolute per-file seconds do not transfer: a two-hour film draws about ten sample windows under the same geometry rather than one.

## Content features against outcomes

**Measured.** Packet-level statistics come from the whole file and cost almost nothing; window features come from one 20 s window inside it.

| Fixture | Bitrate bucket CV | Keyframes | Mean GOP (s) | GOP CV | I-frame size ratio | Packet p90/p50 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| bbb1080_s30 | 0.6181 | 60 | 3.067 | 0.4003 | 46.11 | 7.994 |
| bbb1080_s240 | 0.6540 | 67 | 2.722 | 0.4563 | 39.76 | 7.141 |
| bbb1080_s450 | 0.6612 | 48 | 3.791 | 0.2172 | 24.63 | 9.466 |
| bbb2160_s30 | 0.5913 | 60 | 3.067 | 0.4003 | 42.17 | 6.673 |

| Fixture | MAFD mean | MAFD p95 | Scene-change score p95 | Scene cuts | Entropy diff mean | Entropy diff p95 | Bit-plane noise mean | Detected crop |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| bbb1080_s30 | 0.7956 | 1.9609 | 0.3109 | 4 | 1.1859 | 1.4404 | 0.6057 | 1920x1072 |
| bbb1080_s240 | 1.2697 | 3.9536 | 0.4697 | 5 | 0.9208 | 1.9452 | 0.7380 | 1920x1072 |
| bbb1080_s450 | 3.8872 | 7.3158 | 0.2570 | 0 | 1.1570 | 1.7867 | 0.4913 | 1920x1072 |
| bbb2160_s30 | 0.8303 | 2.0912 | 0.3030 | 4 | 1.1849 | 1.4303 | 0.5272 | 3840x2160 |

**Measured.** Crop detection found the same 8-pixel letterbox on all three 1080p cuts, which is the correction a letterbox-aware bits-per-pixel figure needs and which no baseline predictor currently applies.

**Derived, and not evidence of predictive value.** Ranking the three same-baseline 1080p segments by predicted encode percent gives the order s30, s240, s450. Four of the sixteen feature values above vary monotonically across that ordering: MAFD mean and MAFD p95 rise with it, while the packet-level I-frame size ratio falls and the packet-level bitrate bucket coefficient of variation rises. Everything else, including bitrate, bits per pixel per frame, GOP statistics, scene-cut count, entropy difference, and bit-plane noise, does not.

With three fixtures, any feature unrelated to the outcome ranks them monotonically about a third of the time, so roughly five of sixteen candidates would rank correctly by chance. Four did. This run therefore provides no evidence that any candidate feature predicts anything, and the fact that a free packet-level statistic ranked as well as a 5 s filter pass is a coincidence at this sample size, not a finding.

**Measured, one pair.** The identical-content pair at two resolutions gives a first read on which features survive a resolution change, which matters because features that move with frame size need their analysis parameters recorded alongside them.

| Feature | 1080p value | 2160p value | Change |
| --- | ---: | ---: | ---: |
| Entropy difference mean | 1.1859 | 1.1849 | -0.1% |
| Scene-change score p95 | 0.3109 | 0.3030 | -2.5% |
| MAFD mean | 0.7956 | 0.8303 | +4.4% |
| Bitrate bucket CV | 0.6181 | 0.5913 | -4.3% |
| MAFD p95 | 1.9609 | 2.0912 | +6.6% |
| I-frame size ratio | 46.11 | 42.17 | -8.6% |
| Bit-plane noise mean | 0.6057 | 0.5272 | -13.0% |

**Measured, one pair.** The same content encoded at two resolutions produced best CRF 35.25 at 1080p and 45.25 at 2160p, at predicted encode percents of 69.4 and 44.2. A CRF learned at one resolution does not transfer to another even for identical content, which is consistent with the resolution bucketing the current estimator already does.

The two source encodes differ in more than frame size, since each was encoded independently and their bitrates differ by a factor of two. The change column above therefore mixes true resolution sensitivity with encoder differences and should be treated as a first indication only.

## What this run does not establish

**Measured.** Twenty fixtures were prepared and four were collected. The sixteen outstanding fixtures are the ones that carry the content variety the question depends on: both remaining 4K segments, four segments each from two other films, three re-encodes of pristine masters chosen for heavy motion, two controlled-grain variants, and one deliberately low-headroom recompressed source.

Consequently:

- No held-out prediction comparison against the duration, codec, and resolution baseline exists. Under the group-by-film rule above, four fixtures from one film do not admit even a leave-one-out split.
- No grain, noise, or heavy-motion content was measured, so the one collector justified as a direct grain signal has no fixture that exercises it.
- No non-animated content was measured at all.
- Everything here is one machine, one operating-system environment, and one toolchain, and that toolchain is not the one V3 ships.
- The spatial slot produced no value, so the choice between spatial information and the blur and block detectors is unmeasured.

**Unverified.** The following are carried from earlier investigation and were not reproduced by this run: that scene-change MAFD subsumes temporal information from the spatial-information filter and tracks the VMAF motion feature at window-aggregate level; that spatial information is strongly resolution-dependent and converges at a fixed analysis scale; the relative costs quoted for the blur, block, and signal-statistics collectors; and the behavior of the ab-av1 sample cache key, which hashes an encoder version string that is empty when only FFmpeg's built-in encoder is present, so cached sample results can survive an encoder upgrade unnoticed. Each remains a reason to record tool-version provenance alongside quality and timing observations, and none is admissible as a measured fact.

## Open decisions

- Which discarded attempt and sample facts the adapter retains, and what shape the resulting observation takes.
- Whether the window feature pass runs automatically or on request, given that its cost is a few percent of the search it informs but is spent before the user has asked for a search.
- Whether content features become typed fields or versioned payloads. Features that move with analysis parameters have to carry those parameters, and the resolution pair above is the first measured input to that choice; #96 owns the decision.
- What fills the spatial slot, once a collector for it actually produces values.
- The material-improvement threshold and minimum coverage, which #92 owns and which must be fixed before results are seen, not after.

## Exit criteria for issue #95

- The outstanding fixtures are collected, so that motion, grain, recompression, and non-animated content are represented.
- A held-out comparison against the baseline is reported with error, bias, and coverage, split by source film and by machine and tool version.
- Collection overhead is measured independently of prediction quality, on more than one host.
- Each surviving feature has a stage, a cost, a defined absence behavior, and its analysis parameters recorded with it.
- Facts that this note labels unverified are either measured or dropped.
- The evidence semantics that survive are reconciled with #93 before #96 fixes the logical model and #100 implements collectors.
