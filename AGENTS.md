# Auto-AV1-Converter

GUI application for batch converting videos to AV1 using VMAF-targeted quality encoding via the `ab-av1` tool.

## Tech Stack

- **External tools**: `ab-av1`, FFmpeg with libsvtav1 (downloaded to `vendor/` or system PATH)

## Commands

```bash
python -m src.convert          # Run application (or convert.bat on Windows)
```

Dev tooling (ruff, ty, pytest — run via `uv run`) is declared in `pyproject.toml`.

## Architecture

The conversion pipeline (two-phase encode, VMAF fallback), threading model, callback chain, and data persistence are documented in `docs/ARCHITECTURE.md`.

### Analysis Tab (Four-Level Model)

The Analysis tab allows users to preview conversion estimates before committing to encoding.
Levels are defined in `AnalysisLevel` enum (`src/models.py`) and can be queried via `FileRecord.get_analysis_level()`.

```
Level 0 - DISCOVERED: Folder Scan (on tab open / folder change)
  └── os.scandir() BFS traversal → populates tree with folder/file names
  └── No ffprobe, instant feedback, values show "—"

Level 1 - SCANNED: Basic Scan (on "Basic Scan" button click)
  └── Parallel ffprobe via ThreadPoolExecutor (4-8 workers, 30s timeout)
  └── Updates tree rows with estimated savings/time as results arrive
  └── Uses HistoryIndex cache to skip already-analyzed files
  └── Estimates shown with "~" prefix (e.g., "~1.2 GB")

Level 2 - ANALYZED: Analyze (via queue with ANALYZE operation type)
  └── ab-av1 crf-search on selected files (~1 min/file)
  └── Provides precise CRF and predicted output size
  └── Results shown without "~" prefix (accurate predictions)
  └── Optional - for users who want accurate predictions before encoding

Level 3 - CONVERTED: Convert (via queue processing)
  └── Full SVT-AV1 encoding with optimal CRF
  └── Produces actual output file
```

**Key components**:
- `folder_analysis.py`: `_analyze_file()`, file classification
- `history_index.py`: Thread-safe `HistoryIndex` with O(1) lookups by path hash
- `gui/analysis_controller.py`: `on_add_all_analyze()`, `on_add_all_convert()`, folder change handling
- `gui/analysis_scanner.py`: `incremental_scan_thread()`, `run_ffprobe_analysis()`

**Cache behavior**: Files are cached in `HistoryIndex` by path hash. Cache is validated by file size + mtime. Cached metadata skips ffprobe on subsequent scans.

### Time Estimation

Predicts encoding time from historical data. See `docs/TIME_ESTIMATION.md` for full explanation.

**Predictors**: duration, resolution (bucketed), codec. **NOT file size** - size correlates with bitrate, not encoding complexity.

### Queue System with Operation Types

The queue supports two operation types via `OperationType` enum:

| Operation | What it does | Output |
|-----------|--------------|--------|
| `CONVERT` | Full encoding (includes CRF search if needed) | Video file |
| `ANALYZE` | CRF search only | Updates history (no file) |

**Queue filtering** (`filter_file_for_queue`, `gui/queue_manager.py`): decided verdicts (CONVERTED / NOT_WORTHWHILE / ANALYZED) skip a file only while they still describe the content on disk — a changed file at a known path is re-queueable. The replace-mode output at the input path is recognized without ffprobe via `cache_helpers.converted_verdict_applies()` (see `docs/ARCHITECTURE.md` § Queue Filtering and Verdict Freshness).

**Worker branching** (`sequential_conversion_worker`):
- CONVERT: Calls `process_video()` (existing flow)
- ANALYZE: Calls `wrapper.crf_search()`, updates history with Layer 2 data
- Both: no duplicate detection runs before processing — path-spelling duplicates are unrepresentable after hash-time normalization (ADR-001), and true content copies wait on the partial-hash tier (#28)

**Queue tree updates** (`gui/queue_tree.py`): status/value changes, operation changes, adds, removes, and drag reorders update rows in place (folder expand state, selection, and scroll survive). Full rebuild via `refresh_queue_tree()` is reserved for structural bulk ops (startup load, clear queue, clear completed, conflict replace) and restores expand state. See `docs/ARCHITECTURE.md` § Queue Tree Updates.

## Code Standards

### Strict Rules (not enforced by ruff/ty)
- Log caught exceptions with context - no silent swallowing
- New constants go in `config.py`, not inline
- Prefer creating focused modules over expanding large files
- **Tests** - Unit tests live under `tests/` and run via `uv run pytest`. Changes to pure logic (parsing, formatting, cache/estimation math, etc.) should come with tests. GUI and worker code is exempt until the engine/GUI boundary refactor lands
- **No time estimates** - Never provide effort/duration estimates for tasks
- **No unanonymized log/history access** - Never read `logs/` or `conversion_history.json` unless contents are anonymized (hashed `file_…` names). Real paths identify people; if one appears, stop and don't quote it

### Zero Backwards Compatibility Policy
**NEVER add backwards compatibility code.** This is a single-developer project with no external consumers. Backwards compatibility is wasted effort.

Prohibited patterns:
- **No deprecation shims** - Delete old code immediately, never mark as "deprecated"
- **No renamed variable aliases** - Don't keep `old_name = new_name` mappings
- **No version checks** - Don't branch on versions or feature-detect old behavior
- **No "# removed" comments** - If code is removed, delete it completely with no trace
- **No re-exports for moved code** - When moving functions/classes, update all call sites directly
- **No fallback imports** - Don't try/except import old locations
- **No migration helpers** - Config format changes? Rewrite the config, don't auto-migrate
- **No API preservation** - Function signatures can change freely; update all callers

When refactoring:
1. Make the change directly
2. Update ALL affected code in the same commit
3. Leave no artifacts of the old approach
4. If something breaks, fix it - don't add compatibility layers
5. **Use `git mv` when moving files** - Preserves git history; never delete+create

### Conventions
- **Thread safety**: Never update GUI from worker thread directly. Use `update_ui_safely()`. The worker uses a single-writer model: `queue_item.*` is mutated directly by the one worker thread (UI only reads), `gui.session.*` is mutated via `update_ui_safely` (main thread). See `worker.py:34-44` for details. This is safe—don't add locks.
- **Callbacks**: Events dispatch via `handle_*` functions in `gui/callback_handlers.py`.
- **Exceptions**: Custom hierarchy in `ab_av1/exceptions.py` (InputFileError, OutputFileError, AbAv1CancelledError, ConversionNotWorthwhileError)
- **Persistence**: JSON with atomic writes using `os.replace()`.
- **Process management**: Track PID for graceful/force stop. Use `taskkill /T` on Windows.
- **Error handling**: `except Exception:` + `logger.exception()` is correct for non-critical ops (UI updates, cache writes, metadata extraction). Conversions can run for hours—never abort due to a progress bar glitch. Log everything, continue with safe fallbacks.

## Stdout Parsing

ab-av1 output has two phases with different formats:
- **Quality Detection**: Structured ab-av1 output, reliable progress
- **Encoding**: FFmpeg output, subject to buffering, multiple regex patterns needed

`RUST_LOG` (set in `ab_av1/wrapper.py`) is the only environment variable ab-av1 reads; the leading `debug` level is what enables the parsed output. The `ab_av1=trace`/`ffmpeg=trace` fragments are inert: ab-av1 (vendored 0.11.4) logs at most debug, registers no `ffmpeg` log target, and captures FFmpeg's own output without forwarding it. Encoding progress comes from ab-av1's log lines.

## Privacy & Security

### Path Anonymization

When enabled, file paths and filenames are anonymized with BLAKE2b hashes (configured folders become `[input_folder]`/`[output_folder]` placeholders). Implementation and detection patterns live in `src/privacy.py`.

**Retroactive scrubbing**: Settings tab provides "Scrub Logs" and "Scrub History" buttons to anonymize existing files (irreversible).

**Reverse lookup**: Use `tools/hash_lookup.py` to find files by hash:
```bash
python tools/hash_lookup.py 7f3a9c2b /path/to/videos  # Search by hash prefix
python tools/hash_lookup.py --list .                   # List all file hashes
```

### Other Security Notes

- Never commit `ab_av1_gui_config.json` (may contain paths)
- Process tree termination required for force-stop

## Git

- Branches: `feature/*`, `fix/*`, `refactor/*`
- Run `uv run ruff check src/` before committing

## See Also

- `README.md` - User installation and usage guide
- `docs/ARCHITECTURE.md` - Technical diagrams and data flow
- `docs/TIME_ESTIMATION.md` - How encoding time predictions work
- `docs/AB_AV1_PARSING.md` - How ab-av1/FFmpeg output is parsed
- `docs/HISTORY_FORMAT.md` - Structure of conversion_history.json
- `docs/adr/` - Architecture Decision Records (see `claude.md` within for format rules)
