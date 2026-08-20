# V3 History: sample-attempt and content-complexity evidence research

Status: living research note; not an accepted design or implementation specification

## Purpose and boundary

This note records what the completed measurement passes established about two candidate sources of History evidence. The first is the per-sample and per-attempt facts that ab-av1 produces during a quality search and then discards. The second is content-complexity features that inexpensive FFmpeg collectors can compute before encoding.

It does not select fields, choose between typed content columns and versioned feature payloads, or authorize any collector to run in production.

The measurement harness is standalone research tooling that lives outside this repository and is never committed to it, so its method appears here as prose and its output appears here as tables. The section below records where it lives, because this note is the only durable pointer to it.

## Evidence labels

- **measured**: a value produced by the harness run recorded in this note.
- **measured, single environment**: measured once, on one host and one toolchain, with no repetition and no second machine.
- **derived**: computed from measured values, with the derivation stated.
- **read from source**: established by reading pinned dependency or first-party code rather than by measurement.
- **unverified**: carried from earlier investigation or from reasoning, not reproduced by this run, and not admissible in a contract.

## What the pinned adapter already receives and discards

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
- `pipeline.log`: the run record for the passes reported in this note.

The working area is `~/spike95/` in WSL, holding the pinned toolchain under `bin/`, the cut `fixtures/`, and `scratch/`. All of it is disposable and re-creatable by rerunning `download.sh` and `prep.sh`; only the harness directory itself carries anything worth keeping.

The harness README also fixes the analysis contract. Held-out comparison must group by source film, because segments of one film share style and a naive split leaks between train and test. Prediction targets are best CRF, predicted encode percent, and search wall time. Baseline features are codec, width, height, duration, frame rate, bitrate, and bits per pixel per frame.

## The measured corpus

**Measured, single environment.** Collection ran in two passes on one WSL2 host against fixtures cut from open content (Blender open movies and Xiph derf masters): an initial four-fixture pass and a completion pass covering the remaining sixteen. Every record carries identical recorded toolchain versions, so the twenty records form one comparable corpus. Timings from this environment are meaningful as ratios between passes on the same host, not as absolute costs on user hardware.

| Setting | Value |
| --- | --- |
| Encoder preset | 6 |
| Minimum VMAF | 95.0 |
| ab-av1 sample cache | disabled, so no attempt replayed a cached timing |
| ab-av1 | 0.11.5, invoked as a CLI with `--stdout-format json` |
| FFmpeg/ffprobe | master build `N-126175-g0056dd32fd-20260816` |
| Analysis scale for the spatial pass | 480x270 |
| Sample window duration | 20 s |

**Measured.** Fixture durations run from 10.0 to 183.3 s, and under ab-av1's sampling geometry every fixture contributes exactly one sample window, so nothing in the corpus exercises a window series.

The ground-truth toolchain is not V3's toolchain, and the gap matters when reading the numbers below. V3 pins an ab-av1 0.11.4 fork consumed in process, and `--stdout-format json` postdates that base; the harness used the CLI and its JSON output purely to capture attempts that the in-process adapter would receive as typed updates. FFmpeg was a master build rather than V3's vendored artifact.

### Fixtures

**Measured.** All twenty prepared fixtures are collected: fourteen segments of three open films, three x264 re-encodes of pristine Xiph derf masters, two controlled-grain variants of Sintel content, and one deliberately low-headroom recompressed source.

| Fixture | Category | Codec | Resolution | FPS | Duration (s) | Size (bytes) | Bitrate (kbps) | Bits per pixel per frame |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| bbb1080_s30 | animation 1080p60 | h264 | 1920x1080 | 60 | 182.7 | 75,518,749 | 3,307 | 0.02658 |
| bbb1080_s240 | animation 1080p60 | h264 | 1920x1080 | 60 | 183.3 | 95,100,559 | 4,151 | 0.03336 |
| bbb1080_s450 | animation 1080p60 | h264 | 1920x1080 | 60 | 181.2 | 91,074,971 | 4,021 | 0.03232 |
| bbb2160_s30 | animation 4K60 | h264 | 3840x2160 | 60 | 182.7 | 151,454,805 | 6,632 | 0.01333 |
| bbb2160_s240 | animation 4K60 | h264 | 3840x2160 | 60 | 183.3 | 189,502,727 | 8,272 | 0.01662 |
| bbb2160_s450 | animation 4K60 | h264 | 3840x2160 | 60 | 181.2 | 184,040,719 | 8,125 | 0.01633 |
| bbb_recompressed | already compressed | h264 | 1920x1080 | 60 | 180.0 | 31,035,177 | 1,379 | 0.01109 |
| sintel_s60 | animation film 2K | h264 | 2048x872 | 24 | 180.9 | 43,622,367 | 1,929 | 0.04502 |
| sintel_s300 | animation film 2K | h264 | 2048x872 | 24 | 182.9 | 53,914,773 | 2,358 | 0.05502 |
| sintel_s540 | animation film 2K | h264 | 2048x872 | 24 | 180.9 | 53,965,313 | 2,387 | 0.05569 |
| sintel_s720 | animation film 2K | h264 | 2048x872 | 24 | 178.0 | 70,811,816 | 3,182 | 0.07424 |
| sintel_grain6 | grain mild | h264 | 2048x872 | 24 | 180.0 | 264,679,140 | 11,763 | 0.27446 |
| sintel_grain14 | grain heavy | h264 | 2048x872 | 24 | 180.0 | 1,941,447,064 | 86,285 | 2.01316 |
| tos_s30 | live action low bitrate | h264 | 1920x800 | 24 | 180.1 | 31,920,622 | 1,418 | 0.03847 |
| tos_s240 | live action low bitrate | h264 | 1920x800 | 24 | 181.4 | 38,118,124 | 1,681 | 0.04560 |
| tos_s480 | live action low bitrate | h264 | 1920x800 | 24 | 181.1 | 53,814,533 | 2,377 | 0.06448 |
| tos_s630 | live action low bitrate | h264 | 1920x800 | 24 | 107.3 | 29,187,660 | 2,175 | 0.05901 |
| crowd_run_1080p50_x264 | motion extreme | h264 | 1920x1080 | 50 | 10.0 | 66,458,284 | 53,167 | 0.51280 |
| ducks_take_off_1080p50_x264 | motion detail | h264 | 1920x1080 | 50 | 10.0 | 105,925,835 | 84,741 | 0.81733 |
| old_town_cross_1080p50_x264 | near static | h264 | 1920x1080 | 50 | 10.0 | 60,517,872 | 48,414 | 0.46696 |

## Search outcomes

**Measured.** Seventeen of the twenty searches ran to a successful result.

| Fixture | Best CRF | Achieved VMAF | Predicted encode percent | Predicted output (bytes) | Attempts | Search wall time (s) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| bbb1080_s30 | 35.25 | 95.05 | 69.42 | 50,661,504 | 5 | 199.8 |
| bbb1080_s240 | 37.75 | 95.07 | 50.31 | 47,841,357 | 3 | 129.1 |
| bbb1080_s450 | 58.75 | 95.39 | 9.96 | 8,823,377 | 6 | 220.5 |
| bbb2160_s30 | 45.25 | 95.05 | 44.18 | 66,917,709 | 5 | 745.9 |
| bbb2160_s240 | 44.00 | 95.16 | 39.67 | 75,178,151 | 4 | 787.1 |
| bbb2160_s450 | 62.75 | 95.64 | 5.29 | 9,343,186 | 7 | 1,561.0 |
| bbb_recompressed | failed | failed | failed | failed | 1 | 47.9 |
| sintel_s60 | failed | failed | failed | failed | 5 | 129.4 |
| sintel_s300 | 35.75 | 95.01 | 68.33 | 36,841,955 | 4 | 122.3 |
| sintel_s540 | failed | failed | failed | failed | 4 | 98.3 |
| sintel_s720 | 61.00 | 96.16 | 12.05 | 4,940,931 | 7 | 142.0 |
| sintel_grain6 | 35.50 | 95.08 | 22.11 | 58,512,724 | 5 | 129.4 |
| sintel_grain14 | 27.75 | 95.09 | 11.84 | 204,939,230 | 4 | 117.0 |
| tos_s30 | 30.25 | 95.07 | 73.45 | 21,367,732 | 5 | 96.3 |
| tos_s240 | 29.50 | 95.18 | 70.63 | 19,989,804 | 4 | 68.3 |
| tos_s480 | 35.25 | 95.05 | 63.07 | 29,737,764 | 5 | 84.4 |
| tos_s630 | 68.50 | 95.79 | 5.16 | 1,505,808 | 6 | 92.9 |
| crowd_run_1080p50_x264 | 39.75 | 95.35 | 37.27 | 24,769,150 | 5 | 158.8 |
| ducks_take_off_1080p50_x264 | 32.00 | 95.16 | 57.09 | 60,474,435 | 5 | 161.3 |
| old_town_cross_1080p50_x264 | 35.50 | 95.03 | 5.19 | 3,139,326 | 5 | 132.9 |

**Measured.** All three failures report the same cause, no suitable CRF. In each ladder the VMAF floor was reachable only at predicted sizes above the input: the closest miss on sintel_s540 hit VMAF 95.05 at 153.85 percent of input size. The recompressed fixture fails by design; the two plain Sintel segments fail as ordinary film content whose sources are already efficient. Not-worthwhile outcomes therefore arise inside natural libraries, not only in contrived fixtures.

**Measured.** The three 1080p Big Buck Bunny segments share codec, resolution, frame rate, and duration within 1%, and their bitrates fall inside a 25 percent band. The current estimator groups on codec and a resolution bucket, so it would treat these three files as one cohort and predict one number for all of them. Their actual outcomes span best CRF 35.25 to 58.75 and predicted encode percent 69.42 to 9.96, a factor of seven in predicted output size.

**Measured.** The completion pass shows that spread is not an animation artifact. The four Tears of Steel segments agree on codec, resolution, and frame rate, and their predicted encode percents span 73.45 to 5.16, a factor of fourteen. Of the four plain Sintel segments, two failed outright while the two successes span 68.33 to 12.05 percent, so files one cohort would average differ even in whether a target is reachable.

That spread is the headroom a content signal would have to capture. It exists independently of whether any feature measured below actually captures it.

## Attempt ladders

**Measured.** Ninety-five attempts were produced across the twenty searches: eighty-five by the seventeen successful searches and ten by the three failures. Seventeen, one per successful search, correspond to the `Done` update that V3 retains today. The other seventy-eight, including every attempt of the three failed searches and their 275.6 s of paid search time, are the observations the adapter drops.

**Measured.** Each discarded attempt is a paid-for CRF-to-VMAF observation on known content at a known preset. The searches spent between 47.9 s and 1,561 s producing them, and V3 keeps roughly one in six of what it bought. Per-attempt ladders for every fixture live in the harness results.

**Measured.** Ladders from the first pass show the interpolation model overshooting badly enough to spend an attempt far from the target CRF. The bbb1080_s450 search jumped to CRF 70 and measured VMAF 77.43 against a target of 95, then walked back through 57.75, 58.25, and 58.75. The bbb1080_s30 search jumped down to CRF 18 and measured VMAF 97.86 at a predicted encode percent of 203, meaning a predicted output twice the size of its input. Retained attempt observations are exactly the data a better prior would be fitted on.

**Measured.** Log lines are a worse source than the structured stream for the same run. On the four fixtures of the first pass, parsing ab-av1's stderr recovered boundaries for only 16 of 19 attempts and no per-sample score lines, while structured output carried every attempt. This supports consuming the adapter's typed updates rather than adding a log parser.

## Collector cost

**Measured, single environment.** Three independently timed collector passes ran per fixture. They were a whole-file demux-only packet pass, a sample-window filter pass computing scene change, entropy, bit-plane noise, and crop detection in one decode, and a sample-window spatial-information pass at a fixed 480x270 analysis scale.

| Fixture | Packet pass (s) | Window filter pass (s) | Spatial pass (s) | Search wall time (s) | Packet pass share of search | Window pass share of search |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| bbb1080_s30 | 0.098 | 4.818 | 7.326 | 199.8 | 0.05% | 2.41% |
| bbb1080_s240 | 0.094 | 4.999 | 7.489 | 129.1 | 0.07% | 3.87% |
| bbb1080_s450 | 0.109 | 5.048 | 7.702 | 220.5 | 0.05% | 2.29% |
| bbb2160_s30 | 0.201 | 18.792 | 9.544 | 745.9 | 0.03% | 2.52% |
| bbb2160_s240 | 0.256 | 21.083 | 10.369 | 787.1 | 0.03% | 2.68% |
| bbb2160_s450 | 4.165 | 43.602 | 20.790 | 1,561.0 | 0.27% | 2.79% |
| bbb_recompressed | 0.291 | 5.118 | 8.593 | 47.9 (failed) | 0.61% | 10.68% |
| sintel_s60 | 0.613 | 2.969 | 5.284 | 129.4 (failed) | 0.47% | 2.29% |
| sintel_s300 | 0.483 | 2.034 | 3.938 | 122.3 | 0.39% | 1.66% |
| sintel_s540 | 0.547 | 2.236 | 3.880 | 98.3 (failed) | 0.56% | 2.27% |
| sintel_s720 | 0.778 | 2.202 | 4.362 | 142.0 | 0.55% | 1.55% |
| sintel_grain6 | 1.586 | 2.167 | 3.800 | 129.4 | 1.23% | 1.67% |
| sintel_grain14 | 17.934 | 4.590 | 5.498 | 117.0 | 15.33% | 3.92% |
| tos_s30 | 0.434 | 2.105 | 4.231 | 96.3 | 0.45% | 2.19% |
| tos_s240 | 0.455 | 1.818 | 3.583 | 68.3 | 0.67% | 2.66% |
| tos_s480 | 0.528 | 1.645 | 3.438 | 84.4 | 0.63% | 1.95% |
| tos_s630 | 0.196 | 1.659 | 3.205 | 92.9 | 0.21% | 1.79% |
| crowd_run_1080p50_x264 | 0.423 | 2.204 | 3.827 | 158.8 | 0.27% | 1.39% |
| ducks_take_off_1080p50_x264 | 0.666 | 2.217 | 3.993 | 161.3 | 0.41% | 1.37% |
| old_town_cross_1080p50_x264 | 0.409 | 2.174 | 3.768 | 132.9 | 0.31% | 1.64% |

**Measured.** The spatial pass produced no usable value on any of the twenty fixtures. Every run completed, consumed between 3.2 s and 20.8 s, and recorded an error in place of the expected summary block, so no spatial-information or temporal-information value exists anywhere in the corpus. Whatever this pass cost, the corpus bought nothing with it, and the spatial slot in the feature set remains unmeasured rather than filled.

**Derived.** Window pass cost scales with pixel rate rather than window length. Between 1080p and 2160p cuts of identical content and frame count, the window filter pass rose by factors of 3.90, 4.22, and 8.64 while the search rose by factors of 3.73, 6.10, and 7.08.

**Derived.** The share form is stable for the decode-bound window pass because it and the search scale with the same drivers, sample count and pixel rate. Its share of the informing search stays between 1.37 and 3.92 percent across all seventeen successful searches, spanning animation, film, live action, heavy motion, grain, and three frame sizes. Against the shortest failed search it reached 10.68 percent, because the denominator collapsed after one attempt. Absolute per-file seconds still do not transfer: a two-hour film draws about ten sample windows under the same geometry rather than one.

**Measured.** The packet pass is not uniformly cheap. Its share spans 0.03 to 0.67 percent on moderate-bitrate fixtures, 1.23 percent on the mild-grain variant, and 15.33 percent, 17.93 s, on the 86 Mbps, 1.9 GB heavy-grain fixture.

**Derived.** Packet pass cost tracks container bytes through storage, not search scale. Effective read throughput varied by more than an order of magnitude between similar-sized fixtures (bbb2160_s30 at 0.201 s against bbb2160_s450 at 4.165 s), consistent with page-cache state deciding the cost. Whole-file demux therefore has no stable share of the search it informs: search cost scales with sample count and pixel rate while demux cost scales with file bytes, and high-bitrate content maximizes the ratio. A cold multi-gigabyte source on disk-rate storage costs minutes of pure read time before any search begins.

## Content features against outcomes

**Measured.** Packet-level statistics come from the whole file; window features come from one 20 s window inside it.

| Fixture | Bitrate bucket CV | Keyframes | Mean GOP (s) | GOP CV | I-frame size ratio | Packet p90/p50 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| bbb1080_s30 | 0.6181 | 60 | 3.067 | 0.4003 | 46.110 | 7.994 |
| bbb1080_s240 | 0.6540 | 67 | 2.722 | 0.4563 | 39.758 | 7.141 |
| bbb1080_s450 | 0.6612 | 48 | 3.791 | 0.2172 | 24.630 | 9.466 |
| bbb2160_s30 | 0.5913 | 60 | 3.067 | 0.4003 | 42.167 | 6.673 |
| bbb2160_s240 | 0.5939 | 67 | 2.722 | 0.4563 | 36.187 | 6.335 |
| bbb2160_s450 | 0.6590 | 48 | 3.791 | 0.2251 | 19.949 | 7.950 |
| bbb_recompressed | 0.6679 | 68 | 2.635 | 0.4376 | 39.616 | 9.063 |
| sintel_s60 | 0.6721 | 45 | 3.981 | 0.5999 | 10.975 | 4.519 |
| sintel_s300 | 0.5634 | 47 | 3.942 | 0.6585 | 4.648 | 2.349 |
| sintel_s540 | 0.6405 | 58 | 2.995 | 0.7533 | 6.265 | 3.249 |
| sintel_s720 | 1.0278 | 25 | 7.319 | 0.4654 | 12.331 | 29.864 |
| sintel_grain6 | 0.2418 | 53 | 3.434 | 0.7060 | 6.115 | 1.979 |
| sintel_grain14 | 0.0725 | 43 | 4.252 | 0.8467 | 1.469 | 1.057 |
| tos_s30 | 0.4760 | 47 | 3.884 | 0.3367 | 9.683 | 4.569 |
| tos_s240 | 0.4168 | 57 | 3.217 | 0.4460 | 7.717 | 3.676 |
| tos_s480 | 0.4134 | 50 | 3.631 | 0.4282 | 5.973 | 2.655 |
| tos_s630 | 0.5302 | 23 | 4.703 | 0.1632 | 12.118 | 9.490 |
| crowd_run_1080p50_x264 | 0.0481 | 2 | 5.000 | n/a | 4.373 | 3.282 |
| ducks_take_off_1080p50_x264 | 0.2068 | 2 | 5.000 | n/a | 2.296 | 2.240 |
| old_town_cross_1080p50_x264 | 0.0494 | 2 | 5.000 | n/a | 4.503 | 3.002 |

| Fixture | MAFD mean | MAFD p95 | Scene-change score p95 | Scene cuts | Entropy diff mean | Entropy diff p95 | Bit-plane noise mean | Detected crop |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| bbb1080_s30 | 0.7956 | 1.9609 | 0.3109 | 4 | 1.1859 | 1.4404 | 0.6057 | 1920x1072 |
| bbb1080_s240 | 1.2697 | 3.9536 | 0.4697 | 5 | 0.9208 | 1.9452 | 0.7380 | 1920x1072 |
| bbb1080_s450 | 3.8872 | 7.3158 | 0.2570 | 0 | 1.1570 | 1.7867 | 0.4913 | 1920x1072 |
| bbb2160_s30 | 0.8303 | 2.0912 | 0.3030 | 4 | 1.1849 | 1.4303 | 0.5272 | 3840x2160 |
| bbb2160_s240 | 1.2941 | 3.9768 | 0.4718 | 5 | 0.9265 | 1.9847 | 0.6184 | 3840x2160 |
| bbb2160_s450 | 3.8748 | 7.2278 | 0.1170 | 0 | 1.0796 | 1.7259 | 0.3724 | 3840x2160 |
| bbb_recompressed | 1.0675 | 3.6993 | 0.4578 | 5 | 0.9705 | 1.8345 | 0.6230 | 1920x1072 |
| sintel_s60 | 0.9738 | 1.7598 | 0.1559 | 1 | 1.0111 | 1.9712 | 0.4688 | 2048x864 |
| sintel_s300 | 5.0970 | 10.7619 | 0.8558 | 3 | 0.9178 | 1.4385 | 0.6608 | 2048x864 |
| sintel_s540 | 1.0122 | 2.6269 | 0.2030 | 0 | 1.3604 | 1.7435 | 0.3548 | 2048x864 |
| sintel_s720 | 5.7651 | 6.6117 | 0.0719 | 0 | 0.6018 | 0.7864 | 0.9048 | 2048x864 |
| sintel_grain6 | 5.0001 | 10.7852 | 0.8557 | 3 | 0.8217 | 1.2578 | 0.8662 | 2048x864 |
| sintel_grain14 | 5.6394 | 11.1952 | 0.8161 | 2 | 0.5134 | 0.7762 | 0.9958 | 2048x864 |
| tos_s30 | 1.2778 | 2.5728 | 0.3557 | 2 | 1.3649 | 1.6642 | 0.4055 | 1920x800 |
| tos_s240 | 1.6302 | 3.8727 | 0.6967 | 5 | 1.0606 | 1.4813 | 0.4858 | 1920x800 |
| tos_s480 | 2.6170 | 7.6433 | 0.8313 | 1 | 0.8306 | 0.9948 | 0.6352 | 1920x800 |
| tos_s630 | 12.0848 | 12.7555 | 0.1509 | 1 | 1.8406 | 1.9265 | 0.5582 | 1920x800 |
| crowd_run_1080p50_x264 | 5.1466 | 7.5234 | 0.6541 | 0 | 0.4091 | 0.4296 | 0.9786 | 1920x1072 |
| ducks_take_off_1080p50_x264 | 3.4556 | 4.1649 | 0.2730 | 0 | 0.3589 | 0.3786 | 0.9898 | 1920x1072 |
| old_town_cross_1080p50_x264 | 2.2646 | 2.8966 | 0.6668 | 0 | 0.6389 | 0.6541 | 0.9881 | 1920x1072 |

**Measured.** Crop detection reports 1920x1072 on every 1080p Big Buck Bunny cut, on the recompressed fixture, and on all three Xiph masters, 2048x864 on every Sintel cut including both grain variants, and the full frame on the 4K cuts and Tears of Steel. The letterbox correction a bits-per-pixel figure needs is per-encode rather than global, and no baseline predictor applies it.

**Measured, one ladder.** The controlled-grain variants are the first fixtures to exercise the collector justified as a direct grain signal. On identical content, bit-plane noise rises monotonically with synthetic grain strength: 0.6608 clean, 0.8662 at strength 6, 0.9958 at strength 14. The signal is grain-sensitive rather than grain-specific, because the three pristine high-detail Xiph masters measure 0.9786 to 0.9898 with no added grain.

**Derived, and still not evidence of predictive value.** The first pass's three-fixture monotonic ranking exercise is superseded by the completed corpus and is not repeated here. With six source groups collected (three films and three distinct Xiph masters), a held-out comparison under the group-by-film rule is now possible. It has not been performed, and nothing in this note scores any feature against outcomes.

### Resolution pairs

**Measured, three pairs.** The identical-content pairs at 1080p and 2160p now number three, giving a first read on which features survive a resolution change, which matters because features that move with frame size need their analysis parameters recorded alongside them.

| Feature | s30 pair | s240 pair | s450 pair |
| --- | ---: | ---: | ---: |
| Entropy difference mean | -0.1% | +0.6% | -6.7% |
| Scene-change score p95 | -2.5% | +0.4% | -54.5% |
| MAFD mean | +4.4% | +1.9% | -0.3% |
| MAFD p95 | +6.6% | +0.6% | -1.2% |
| Bitrate bucket CV | -4.3% | -9.2% | -0.3% |
| I-frame size ratio | -8.6% | -9.0% | -19.0% |
| Bit-plane noise mean | -13.0% | -16.2% | -24.2% |

**Measured, three pairs.** MAFD mean and entropy difference mean hold within about 7 percent across all three pairs. Bit-plane noise falls at 4K in every pair, by 13 to 24 percent, so it moves with frame size. The scene-change score is unstable where its absolute value is small, swinging 54.5 percent on the s450 pair.

**Measured, three pairs.** Best CRF moved from 35.25 to 45.25, from 37.75 to 44.00, and from 58.75 to 62.75 between 1080p and 2160p on identical content. A CRF learned at one resolution does not transfer to another, even for identical content. This agrees with the current estimator's resolution bucketing.

The paired source encodes differ in more than frame size, since each was encoded independently and their bitrates differ by about a factor of two. The change columns above therefore mix true resolution sensitivity with encoder differences and should be treated as a first indication only.

## What the corpus does not establish

**Measured.** All twenty prepared fixtures are collected. What remains unestablished:

- No held-out prediction comparison against the duration, codec, and resolution baseline has been run. The completed corpus admits one under the group-by-film rule, with six source groups.
- Everything here is one machine, one operating-system environment, and one toolchain, and that toolchain is not the one V3 ships.
- The spatial slot produced no value on any fixture, so the choice between spatial information and the blur and block detectors is unmeasured.
- No fixture draws more than one sample window, so multi-window scaling of collector cost on long content stays derived rather than measured.
- Packet pass cost under controlled cold and warm cache states was observed incidentally, on two fixtures, not measured systematically.

**Unverified.** Earlier investigation claimed that scene-change MAFD subsumes temporal information from the spatial-information filter and tracks the VMAF motion feature at window-aggregate level. It also claimed that spatial information is resolution-dependent and converges at a fixed analysis scale, and it quoted relative costs for the blur, block, and signal-statistics collectors. This run did not reproduce those claims. It also did not verify the ab-av1 sample cache key behaviour, where an empty built-in encoder version may let cached samples survive an encoder upgrade. Each claim supports recording tool-version provenance alongside quality and timing observations, but none is admissible as a measured fact.

## Open decisions

- Which discarded attempt and sample facts the adapter retains, and what shape the resulting observation takes.
- Whether the window feature pass runs automatically or on request, given that its cost is a few percent of the search it informs but is spent before the user has asked for a search.
- Whether the whole-file packet pass survives at all, given byte-scaled cost on high-bitrate sources, or becomes a bounded-read collector, and what bound it gets.
- Whether content features become typed fields or versioned payloads. Features that move with analysis parameters have to carry those parameters, and the resolution pairs above are the first measured input to that choice, which the History logical model owns.
- What fills the spatial slot, once a collector for it actually produces values.
- The material-improvement threshold and minimum coverage, which must be fixed before results are seen, not after. The evaluation contract they belong to is in `docs/design/estimation.md`.

## Exit criteria

- A held-out comparison against the baseline is reported with error, bias, and coverage, split by source film and by machine and tool version.
- Collection overhead is measured independently of prediction quality, on more than one host, including packet pass runs under controlled cold and warm cache states.
- Each surviving feature has a stage, a cost, a defined absence behaviour, and its analysis parameters recorded with it.
- Facts that this note labels unverified are either measured or dropped.
- The evidence semantics that survive are reconciled with the evidence-quality contract before the History logical model is fixed and any collector is implemented.
