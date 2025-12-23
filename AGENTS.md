# Auto-AV1-Converter

GUI application for batch converting videos to AV1 using VMAF-targeted quality encoding via the `ab-av1` tool.

## Tech Stack

- **Python 3** with Tkinter GUI
- **UV** for package management
- **Ruff** for linting/formatting
- **ty** for type checking
- **External tools**: `ab-av1` (in `src/`), FFmpeg with libsvtav1 (system PATH)

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
├── models.py                  # Dataclasses (ProgressEvent, ConversionConfig, FileRecord, etc.)
├── estimation.py              # Time estimation from history
├── utils.py                   # Logging, formatting, ffprobe, privacy helpers
├── video_conversion.py        # Single-file conversion logic
├── folder_analysis.py         # Analysis tab: scanning, estimation, file classification
├── history_index.py           # Thread-safe O(1) cache for FileRecord lookups
├── ab_av1/                    # ab-av1 wrapper package
│   ├── wrapper.py             # Subprocess management, VMAF fallback
│   ├── parser.py              # Regex parsing of ab-av1/ffmpeg output
│   ├── exceptions.py          # Custom exception hierarchy
│   ├── checker.py             # ab-av1 availability check
│   └── cleaner.py             # Temp folder cleanup
├── conversion_engine/         # Batch conversion (no GUI imports)
│   ├── worker.py              # Sequential worker thread
│   └── scanner.py             # Video file scanning/filtering
└── gui/                       # Tkinter GUI
    ├── main_window.py         # Main window, settings persistence
    ├── conversion_controller.py # Start/stop/force-stop logic, callback dispatcher
    ├── callback_handlers.py   # Event handlers (progress, completed, error, etc.)
    ├── gui_updates.py         # Thread-safe UI updates
    ├── gui_actions.py         # User interaction handlers
    └── tabs/                  # Tab implementations
        └── analysis_tab.py    # Analysis tab UI definition

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

### Analysis Tab (Three-Phase Model)

The Analysis tab allows users to preview conversion estimates before committing to encoding.

```
Phase 1: Incremental Scan (on tab open / folder change)
  └── os.scandir() BFS traversal → populates tree with folder/file names
  └── No ffprobe, instant feedback, values show "—"

Phase 2: Metadata Analysis (on "Analyze" button click)
  └── Parallel ffprobe via ThreadPoolExecutor (4-8 workers, 30s timeout)
  └── Updates tree rows with estimated savings/time as results arrive
  └── Uses HistoryIndex cache to skip already-analyzed files

Phase 3: Quality Analysis (on "Analyze Quality" button click)
  └── ab-av1 crf-search on selected files (~1 min/file)
  └── Provides precise CRF and predicted output size
  └── Optional - for users who want accurate predictions before encoding
```

**Key components**:
- `folder_analysis.py`: `scan_folder_fast()`, `_analyze_file()`, file classification
- `history_index.py`: Thread-safe `HistoryIndex` with O(1) lookups by path hash
- `main_window.py`: `_run_analysis_thread()`, `_run_quality_analysis_thread()`

**Cache behavior**: Files are cached in `HistoryIndex` by path hash. Cache is validated by file size + mtime. Cached metadata skips ffprobe on subsequent scans.

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
- **Thread safety**: Never update GUI from worker thread directly. Use `update_ui_safely()`.
- **Callbacks**: Events dispatch via `handle_*` functions in `gui/callback_handlers.py`.
- **Exceptions**: Custom hierarchy in `ab_av1/exceptions.py` (InputFileError, OutputFileError, VMAFError, etc.)
- **Persistence**: JSON with atomic writes using `os.replace()`.
- **Process management**: Track PID for graceful/force stop. Use `taskkill /T` on Windows.

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
| `conversion_history_v2.json` | File records: metadata, analysis results, conversion history |
| `logs/*.log` | Rotating log files |

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

**Implementation** (`src/utils.py`):
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
