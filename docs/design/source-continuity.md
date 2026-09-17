# Source continuity and output settlement

Status: selected alpha contract; implementation gaps and unresolved observation details are explicit below.

## Contract and ownership

A run remains tied to its prepared source evidence across search, encode or remux, retries, and output settlement. Source continuity means that the required observations detected no disqualifying change across a defined processing interval. It does not prove that every byte stayed unchanged between observations.

ADR-019 owns content identity and its limits. ADR-020 owns output publication, destructive authorization, and recovery. This contract extends those decisions; it does not select a new storage engine or an immutable-input mechanism.

The alpha policy rejects ordinary success when required source evidence changes or cannot be assessed. It preserves a verified output with a typed reason when settlement cannot proceed. A source conflict affects that item; unrelated queue work can continue once processes and output ownership are settled. Uncertain journal durability remains a driver failure under the existing durability contract.

## Shipped gaps

The prepared `JobSpec` retains a content key but no immutable source filesystem observation. The coordinator records a search result before output planning, which inspects the source again without comparing it with the evidence that justified the search. A later source can therefore become the transaction baseline while the run still names the earlier content.

`recover_ready` checks staging and the destination preimage but not the original source. Distinct-path output can promote after source replacement, disappearance, or metadata change. Retirement rejects a changed present original, but accepts an absent original without distinguishing external disappearance from deletion after a durable retirement intent.

`Replacement::KeepOriginal` also represents same-path replacement of a Matroska input. It means that no separate retirement step is required. Source-preserving output, same-path promotion, and separate retirement need distinct policy treatment.

`AnalysisRecorded` immediately admits results to the reusable analysis index. Completion evidence and History projections obtain input size from the transaction or other fallbacks without a source-continuity qualification. `Conflict` retains a reason but no structured artifact identity in its folded state. These gaps require engine, core, recovery, and consumer changes together.

## Separate judgments

| Judgment | Evidence and authority |
| --- | --- |
| Process completion | The supervised operation's terminal report |
| Artifact verification | The specified checks on the produced file; an AV1 probe and minimum size do not establish full decode correctness |
| Source continuity | Observations bound to the prepared source and the relevant phase |
| Destructive authorization | Fresh observations of the exact paths and objects affected by promotion or retirement |
| Reusable evidence | The core policy combining phase assessment, content identity, and execution profile |

The types must encode permitted combinations rather than independent flags that allow an unassessed success. Filesystem observations originate in the engine; the core decides their consequences. Live commands and durable recovery enforce the same predicates.

## Source observations

Observation classes describe available evidence, not an inferred sequence of filesystem calls. A different file ID means the path resolves to another object; it does not prove an atomic rename occurred. A changed timestamp does not prove that media bytes changed.

| Observation | Meaning |
| --- | --- |
| Required fields match | No difference detected by those observations |
| File identity differs | The path resolves to a different filesystem object |
| Same identity, changed size or timestamps | An observed property changed |
| Path absent | The source is unavailable at that path |
| Inspection failed or required fields unavailable | The required assessment cannot be established |

Identity fields must describe one coherent object observation. Combining size and timestamps from one path lookup with a file ID from another permits an identity assembled from different objects. Opening and inspecting one handle improves coherence, but a CRFty-owned handle does not prove that an external process opened that same object.

Change-time evidence merits evaluation alongside modification time. Unix `ctime` can detect writes followed by restoring `mtime`, but permissions and ownership changes also affect it. Windows `ChangeTime` and `LastWriteTime` are distinct fields. Neither platform supplies an assumed, universal continuity guarantee across all filesystems. [Borg's observation choices][borg-doc], [restic's change detection][restic], and [Microsoft's field definitions][windows-basic] establish these distinctions.

Change-time evidence belongs to source assessment, not the sampled content digest. Adding it to every destructive identity comparison without considering application-owned renames could invalidate legitimate output transitions.

## Phase boundaries and reuse

The prepared source baseline remains authoritative. Search, output planning, and retries validate it; they do not replace it with whatever occupies the path later. Processing a changed source requires fresh preparation.

Search publication requires an assessment of its own read interval. A detected change prevents reusable analysis and a decisive not-worthwhile verdict. Retained attempt evidence stays distinct from evidence eligible to suppress work. A later matching observation cannot erase an earlier detected discontinuity in that interval.

Encode and remux validate the source before starting and after their readers settle. Hardware-to-software retries also validate the baseline. A valid search can remain associated with its original content when only a later encode interval fails assessment; a later failure does not invalidate earlier, independently qualified evidence.

Source assessment precedes publication, and paths affected by promotion or retirement still need fresh destructive checks. A result qualified for its processing interval and permission to mutate a path are separate facts. Cancellation remains reachable during any process-based inspection.

## Output dispositions

| Condition | Required disposition |
| --- | --- |
| Required assessments pass and output verifies | Permit settlement subject to destination and retirement guards |
| Source changes before a phase starts | Fail the run without processing the new source under the old evidence |
| Source changes during search | Reject reusable publication; retain truthful attempt evidence |
| Source changes during encode or remux and verified staging exists | Retain the artifact durably, report source conflict, and block promotion and retirement |
| Required inspection fails | Report assessment failure distinctly from detected mutation; preserve owned artifacts whose disposition cannot be established |
| A disqualifying change is detected after promotion | Preserve the promoted output and present source; block further destructive action and report conflict |
| Source is absent when recovering an authorized retirement intent | Reconcile the requested absence using durable evidence; do not invent who removed the source or when |
| Source changes after a completed durable run | Preserve historical facts and reassess current applicability |

The matrix applies to suffix, separate-folder, same-path replacement, and replacement with separate retirement. Path aliases and hardlinks require filesystem identity checks; different path strings do not establish different objects.

Verified retained artifacts need durable identity, location, verification status, ownership, and retention reason. They must survive replay and compaction without becoming ordinary successes or disposable partial files. The existing abandonment path deletes staging and cannot represent this outcome. A generic conflict string alone cannot preserve the required artifact facts.

Same-path promotion replaces the source pathname itself. Source evidence needed to qualify the result must be durable before that action. Recovery can then recognize a completed rename without trying to reconstruct the vanished input observation. Missing source evidence cannot be filled in by inspecting the replacement output.

The alpha Queue error must explain the conflict and identify any retained artifact. General History failure filters can remain deferred, but retained output must be inspectable without those filters. A full recovery interface and automatic adoption of conflicted artifacts are separate capabilities.

## History and consumer eligibility

Pre-run source size, verified artifact size, eligible reduction statistics, and eligible prediction/actual pairs are different facts. An observed source size remains an observation even when continuity fails; it cannot stand in for a qualified input measurement. Unknown or disqualified values must not be refilled by projection fallbacks.

Uncertain results do not create converted verdicts, suppress future work, enter ordinary success aggregates, or qualify as prediction/actual pairs for estimation. Valid phase evidence remains available with its assessment. The History observation contract consumes these facts without changing output ownership or requiring a storage redesign to implement the source checks.

## Comparable implementations

These references support individual design choices. They do not establish that another project provides CRFty's complete contract.

- [rsync][rsync] separates successful transfer from source removal, rejects removal after a size or modification-time change, and documents the assumption of inactive source files. CRFty likewise needs independent destructive authorization after encoding.
- [Borg 1.4.5][borg-source] checks the opened descriptor after reading on Unix, marks changed files, and excludes those results from its unchanged-file cache. That supports retaining evidence without allowing reuse. This release's Windows branch leaves that during-read check unimplemented; platform support alone is not parity evidence.
- [restic][restic] distinguishes metadata-based reuse from reading a filesystem snapshot through Windows Volume Shadow Copy Service. Stronger input isolation is a separate capability, not a claim supplied by metadata checks.
- [Syncthing][syncthing] stages writes, retains interrupted temporary transfers, and preserves conflict copies. Its temporary namespace is excluded from synchronization. CRFty needs explicit ownership and discovery treatment for retained staging; Syncthing's retention period is not a suitable default for potentially unique conversion output.
- [Unmanic's postprocessor][unmanic] uses partial destinations and removes sources after successful file movement. Its helper removes an existing destination before final placement. This is a publication precedent, not a replacement for journaled crash recovery.
- [Tdarr's replacement plugin][tdarr] stages output, renames the original aside, and attempts restoration after a caught placement failure. Its exception handling does not by itself establish process-crash recovery. CRFty retains the transaction selected by ADR-020 rather than adopting another replacement sequence.

## Validation boundaries

Pure tests cover observation classes, phase eligibility, output modes, and live/replay agreement. Real-process tests synchronize around opens, reads, retries, and completion; sleeps do not establish which source the child consumed. Each mutation test verifies that the attempted filesystem mutation succeeded before interpreting the result.

Crash cases cover readiness before promotion, rename before commit acknowledgement, retirement intent before deletion acknowledgement, and settlement before terminal recording. Replay and compacted snapshots must preserve assessment and retained-artifact facts. Same-path recovery must not interpret application-owned replacement as an external input mutation.

Filesystem cases include same-size writes, restored modification times, metadata-only changes, missing or coarse timestamps, inspection failures, hardlink writes, path aliases, and replacement followed by restoration. Tests for changes invisible to the selected observations document the limitation rather than asserting detection. Windows and Linux require separate executable evidence.

## Unresolved observation details

- Which coherent metadata acquisition API and change-time fields satisfy the contract on each supported platform without first-party unsafe code?
- Which missing or coarse fields prevent qualification, and which weaker observations remain acceptable under an explicitly stated policy?
- Which phase observations need their own durable representation, and which can be carried atomically with result publication or output readiness?
- What typed retained-artifact state preserves ownership after conflict without allowing later automatic promotion or cleanup?

An immutable-input guarantee requires a distinct decision about snapshots or other input isolation. Full-file hashing, mandatory copies, watchers, and settling delays are not selected mechanisms. Source mutation does not trigger an automatic rerun against a new baseline.

[rsync]: https://download.samba.org/pub/rsync/rsync.1#--remove-source-files
[borg-doc]: https://borgbackup.readthedocs.io/en/1.4.5/usage/create.html
[borg-source]: https://github.com/borgbackup/borg/blob/1.4.5/src/borg/archive.py
[restic]: https://restic.readthedocs.io/en/stable/040_backup.html#file-change-detection
[windows-basic]: https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_basic_info
[syncthing]: https://docs.syncthing.net/users/syncing
[unmanic]: https://github.com/Unmanic/unmanic/blob/1c324b8fc3974ffce3d7cc945adb938fe7182910/unmanic/libs/postprocessor.py
[tdarr]: https://github.com/HaveAGitGat/Tdarr_Plugins/blob/26c97a52f9dcf5fc6faeb751071cb82cdf97ca4e/FlowPluginsTs/CommunityFlowPlugins/file/replaceOriginalFile/1.0.0/index.ts
