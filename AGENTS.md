# Auto-AV1-Converter

GUI application for batch converting videos to AV1 using VMAF-targeted quality encoding via the `ab-av1` tool.

## Tech Stack

- **Python 3** with Tkinter GUI
- **UV** for package management
- **Ruff** for linting/formatting
- **ty** for type checking
- **External tools**: `ab-av1`, FFmpeg with libsvtav1 (downloaded to `vendor/` or system PATH)

## Commands

```bash
# Run application
python -m src.convert          # or convert.bat (Windows)

# Development
uv sync                        # Install dev dependencies
uv run ruff check src/         # Lint
uv run ruff check --fix src/   # Lint with auto-fix
uv run ruff format src/        # Format
uv run ty check src/           # Type check
```

## Project Structure

```
src/
├── convert.py                 # Entry point
├── main.py                    # App initialization, Tkinter setup
├── config.py                  # Constants (VMAF targets, presets)
├── models.py                  # Dataclasses (ProgressEvent, ConversionConfig, FileRecord, QueueItem, OperationType, etc.)
├── estimation.py              # Time estimation from history
├── utils.py                   # Formatting helpers, ffprobe, UI thread safety
├── video_conversion.py        # Single-file conversion logic
├── folder_analysis.py         # Analysis tab: scanning, estimation, file classification
├── history_index.py           # Thread-safe O(1) cache for FileRecord lookups
├── cache_helpers.py           # CRF cache validation and reuse logic
├── logging_setup.py           # Logging configuration with rotating handlers
├── platform_utils.py          # Windows subprocess hiding, power management
├── privacy.py                 # Path anonymization (BLAKE2b hashing)
├── hardware_accel.py          # Hardware-accelerated decoding (CUVID, QSV)
├── video_metadata.py          # Video metadata extraction from ffprobe
├── vendor_manager.py          # ab-av1/FFmpeg download and update management
├── ab_av1/                    # ab-av1 wrapper package
│   ├── wrapper.py             # Subprocess management, VMAF fallback
│   ├── parser.py              # Regex parsing of ab-av1/ffmpeg output
│   ├── exceptions.py          # Custom exception hierarchy
│   ├── checker.py             # ab-av1 availability check
│   └── cleaner.py             # Temp folder cleanup
├── conversion_engine/         # Batch conversion (no GUI imports)
│   ├── worker.py              # Sequential worker thread
│   ├── scanner.py             # Video file scanning/filtering
│   └── cleanup.py             # Temp folder cleanup scheduling
└── gui/                       # Tkinter GUI
    ├── main_window.py         # Main window, settings persistence
    ├── base.py                # Base GUI components (explorer, tooltips)
    ├── constants.py           # Centralized UI colors, fonts, styling
    ├── conversion_controller.py # Start/stop/force-stop logic, callback dispatcher
    ├── analysis_controller.py # Analysis tab coordination/events
    ├── queue_controller.py    # Queue tab event handling
    ├── callback_handlers.py   # Event handlers (progress, completed, error, etc.)
    ├── gui_updates.py         # Thread-safe UI updates
    ├── gui_actions.py         # User interaction handlers
    ├── analysis_scanner.py    # Incremental folder scanning with ffprobe
    ├── analysis_tree.py       # Analysis tree display/state management
    ├── queue_manager.py       # Queue item creation/categorization
    ├── queue_tree.py          # Queue tree display/state management
    ├── tree_utils.py          # Tree expand/collapse utilities
    ├── tree_display.py        # Shared tree status formatting
    ├── tree_formatters.py     # Time/size/efficiency formatting and parsing
    ├── dependency_manager.py  # ab-av1/FFmpeg version checking and updates
    ├── charts.py              # Canvas-based chart drawing (bar, pie, line)
    ├── tabs/                  # Tab implementations
    │   ├── analysis_tab.py    # Analysis tab UI definition
    │   ├── convert_tab.py     # Convert tab with queue and progress
    │   ├── settings_tab.py    # Settings tab
    │   └── statistics_tab.py  # Statistics/history tab
    ├── dialogs/               # Modal dialog windows
    │   └── ffmpeg_download_dialog.py  # FFmpeg download confirmation
    └── widgets/               # Reusable UI components
        ├── operation_dropdown.py   # In-cell operation dropdown for queue
        └── add_to_queue_dialog.py  # Preview dialog for queue additions

tools/
└── hash_lookup.py             # Reverse lookup for anonymized file hashes
```

## Architecture

### Two-Phase Conversion

1. **Quality Detection**: ab-av1 samples video at various CRF values to find one meeting VMAF target
2. **Encoding**: FFmpeg encodes full video with optimal CRF

### VMAF Fallback

If target VMAF (default 95) is unattainable, decrements by 1 down to minimum (90), then skips as "not worthwhile".

### Threading Model

- **Main thread**: Tkinter event loop
- **Worker thread**: `sequential_conversion_worker()` handles conversion
- **Analysis threads**: `ThreadPoolExecutor` with 4-8 parallel ffprobe workers
- **GUI updates**: All UI changes via `utils.update_ui_safely()` → `root.after()`

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

**Queue display logic** (Operation column):
- `ANALYZE` type → shows "Analyze"
- `CONVERT` type + has Layer 2 data → shows "Convert"
- `CONVERT` type + no Layer 2 data → shows "Analyze+Convert"

**Analysis tab toolbar**:
- "Basic Scan" → runs ffprobe on discovered files
- "Add All: Analyze" → adds all files to queue with ANALYZE operation
- "Add All: Convert" → adds all files to queue with CONVERT operation

**Context menu options** (Analysis tab):
- "Add to Queue: Convert" / "Add to Queue: Analyze" for individual files/folders

**Context menu options** (Queue tab):
- "Open File" / "Open in Explorer" for files/folders
- Operation options: directly change between "Analyze + Convert", "Convert", "Analyze Only"
- "Remove" to remove from queue

**Properties panel behavior**:
- CONVERT items: Show output mode, suffix, folder settings
- ANALYZE items: Disable output settings (no output file produced)

**Worker branching** (`sequential_conversion_worker`):
- CONVERT: Calls `process_video()` (existing flow)
- ANALYZE: Calls `wrapper.crf_search()`, updates history with Layer 2 data

### Callback Flow

```
AbAv1Wrapper.auto_encode()
  → parser.parse_line()
  → file_callback_dispatcher()
  → handle_* functions (progress, completed, error, skipped)
  → gui_updates.* functions
  → update_ui_safely() → Tkinter main thread
```

## Code Standards

### Strict Rules (not enforced by ruff/ty)
- Log caught exceptions with context - no silent swallowing
- New constants go in `config.py`, not inline
- Prefer creating focused modules over expanding large files
- **No tests** - This project does not use automated testing
- **No time estimates** - Never provide effort/duration estimates for tasks
- **No git commits** - AI assistants must never run `git add`, `git commit`, or `git push`. The user handles all git operations.

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
- **Exceptions**: Custom hierarchy in `ab_av1/exceptions.py` (InputFileError, OutputFileError, VMAFError, etc.)
- **Persistence**: JSON with atomic writes using `os.replace()`.
- **Process management**: Track PID for graceful/force stop. Use `taskkill /T` on Windows.
- **Error handling**: `except Exception:` + `logger.exception()` is correct for non-critical ops (UI updates, cache writes, metadata extraction). Conversions can run for hours—never abort due to a progress bar glitch. Log everything, continue with safe fallbacks.

## Configuration

Key constants in `src/config.py`:

| Constant | Default | Purpose |
|----------|---------|---------|
| `DEFAULT_VMAF_TARGET` | 95 | Quality target (0-100) |
| `DEFAULT_ENCODING_PRESET` | 6 | SVT-AV1 speed preset |
| `MIN_VMAF_FALLBACK_TARGET` | 90 | Lowest VMAF before skipping |
| `MIN_RESOLUTION_WIDTH/HEIGHT` | 1280×720 | Minimum resolution filter |

## Stdout Parsing

ab-av1 output has two phases with different formats:
- **Quality Detection**: Structured ab-av1 output, reliable progress
- **Encoding**: FFmpeg output, subject to buffering, multiple regex patterns needed

See `ab_av1/wrapper.py` for environment variables that maximize verbosity.

## Data Files

| File | Purpose |
|------|---------|
| `av1_converter_config.json` | User settings (managed via GUI) |
| `conversion_history.json` | File records: metadata, analysis results, conversion history |
| `logs/*.log` | Rotating log files |
| `vendor/` | Downloaded ab-av1 and FFmpeg binaries (gitignored) |

**History/Index usage**:
- **Time estimation**: Find similar files (codec/resolution/duration) to predict encoding time
- **Analysis cache**: Skip ffprobe for files with valid cached metadata (size + mtime match)
- **Status tracking**: Track file states (SCANNED, CONVERTED, NOT_WORTHWHILE)

## Privacy & Security

### Path Anonymization

When enabled, file paths and filenames are anonymized using BLAKE2b hashes:

| Original | Anonymized |
|----------|------------|
| `C:\Videos\movie.mp4` | `folder_7f3a9c2b1e4d/file_8a4b2c1d3e5f.mp4` |
| Configured input folder | `[input_folder]/file_8a4b2c1d3e5f.mp4` |
| Configured output folder | `[output_folder]/file_1a2b3c4d5e6f.mkv` |

**Implementation** (`src/privacy.py`):
- `anonymize_file(filename)` - Hashes filename (basename only)
- `anonymize_folder(path)` - Hashes folder path, or returns `[input_folder]`/`[output_folder]` for configured directories
- `anonymize_path(full_path)` - Combines folder + file anonymization
- `PathPrivacyFilter` - Log filter that proactively detects and anonymizes paths via regex

**Patterns detected**:
- Windows paths (`C:\...`, `C:/...`)
- UNC paths (`\\server\share\...`)
- Unix paths (`/home/...`, `/mnt/...`)
- Video filenames (`.mp4`, `.mkv`, `.avi`, `.wmv`, `.mov`, `.webm`)

**Retroactive scrubbing**: Settings tab provides "Scrub Logs" and "Scrub History" buttons to anonymize existing files (irreversible).

**Reverse lookup**: Use `tools/hash_lookup.py` to find files by hash:
```bash
python tools/hash_lookup.py 7f3a9c2b /path/to/videos  # Search by hash prefix
python tools/hash_lookup.py --list .                   # List all file hashes
```

### Other Security Notes

- Never commit `av1_converter_config.json` (may contain paths)
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
