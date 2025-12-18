# Refactoring Plan

## Current State Assessment

The codebase is **better organized than initially assumed**:
- Worker uses callback dispatcher pattern (not direct GUI calls)
- GUI updates centralized in `gui_updates.py`
- Main window delegates to modules (247 lines, not a massive god object)

**Actual problems:**
1. **Thread-unsafe mutations**: `callback_handlers.py` mutates `gui.*` lists/counters directly (not via `update_ui_safely`)
2. **Backwards dependency**: `conversion_engine/` imports from `gui/` (should be opposite)
3. **Long complex functions**: `estimate_remaining_time` (193 LOC), `update_conversion_statistics` (159 LOC)
4. **No typed data structures**: Dicts with inconsistent keys passed everywhere

---

## Phase 0: Thread Safety Fixes
**Risk: Low | App remains functional: Yes**

### Tasks
- [ ] Audit `callback_handlers.py` for direct `gui.*` mutations outside `update_ui_safely`
  - Line 95: `gui.error_count += 1`
  - Line 150: `gui.vmaf_scores.append(vmaf_float)`
  - Line 159: `gui.crf_values.append(crf_int)`
  - Line 170: `gui.size_reductions.append(size_reduction)`
  - Lines 187-189: `gui.total_*` increments
  - Line 219: `gui.skipped_not_worth_count += 1`
  - Line 226: `gui.skipped_not_worth_files.append(filename)`
- [ ] Wrap all mutations in `update_ui_safely` calls
- [ ] Audit `worker.py` lines 75-82 for same issue
- [ ] Fix force-stop PID race in `conversion_controller.py:264`
  - Currently clears `gui.current_process_info = None` BEFORE kill attempt
  - If worker sets new PID between read (262) and clear (264), new process orphans
  - Fix: Clear AFTER kill attempt completes (move to ~line 304)
- [ ] Wrap `gui.pending_files.remove()` in `update_ui_safely()` at `worker.py:329`
  - Main thread iterates `pending_files` in `utils.py:668` (`estimate_remaining_time`)
  - Concurrent remove during iteration → RuntimeError or corrupted state
- [ ] Test: Run conversion, verify no race conditions in stats display

---

## Phase 1: Data Models
**Risk: Low | App remains functional: Yes**

Create typed dataclasses to replace ad-hoc dicts. This enables type-safe refactoring in Phases 2-3: the type checker will catch broken imports and signature mismatches when moving files and extracting functions.

### Tasks
- [ ] Create `src/models.py` with:
  ```python
  @dataclass
  class ProgressEvent:
      filename: str
      phase: str  # "crf-search" | "encoding"
      quality_percent: float
      encoding_percent: float
      vmaf: Optional[float]
      crf: Optional[int]
      eta_text: Optional[str]
      message: str

  @dataclass
  class ConversionResult:
      input_path: str
      output_path: str
      elapsed_seconds: float
      input_size_bytes: int
      output_size_bytes: int
      final_crf: int
      final_vmaf: float

  @dataclass
  class ConversionConfig:
      input_folder: str
      output_folder: str
      extensions: list[str]
      overwrite: bool
      convert_audio: bool
      audio_codec: str
      delete_original: bool
  ```
- [ ] Update `callback_handlers.py` to use `ProgressEvent` instead of `dict`
- [ ] Update `handle_completed` to use `ConversionResult`
- [ ] Update `sequential_conversion_worker` signature to accept `ConversionConfig`
- [ ] Test: Full conversion cycle with new types

---

## Phase 2: Fix Backwards Dependencies
**Risk: Medium | App remains functional: Yes (incremental)**

The `conversion_engine/` package should not import from `gui/`.

### Current Problem
```
conversion_engine/callback_handlers.py imports gui/gui_updates.py  # Wrong direction
conversion_engine/worker.py imports gui/gui_updates.py              # Wrong direction
```

### Tasks
- [ ] Move `callback_handlers.py` from `conversion_engine/` to `gui/`
  - It updates GUI, so it belongs in GUI layer
  - Update imports in `worker.py`
- [ ] Remove `gui_updates` import from `worker.py` (lines 43-46)
  - Worker should only use callbacks passed as parameters
  - Move any direct `gui_updates.*` calls to callback dispatcher
- [ ] Verify import graph: `conversion_engine/` should only import from `ab_av1/`, `utils`, `config`
- [ ] Test: Full conversion cycle

---

## Phase 3: Extract Long Functions
**Risk: Low | App remains functional: Yes**

### Priority 1: `utils.py:estimate_remaining_time` (193 lines)
- [ ] Create `src/estimation.py`
- [ ] Move `find_similar_file_in_history()` to `estimation.py` (dependency)
- [ ] Move `estimate_processing_speed_from_history()` to `estimation.py` (dependency)
- [ ] Extract `estimate_current_file_eta(gui) -> float`
- [ ] Extract `estimate_pending_files_eta(gui, pending_files) -> float`
- [ ] Extract `get_file_processing_estimate(path, history) -> float`
- [ ] Simplify main function to orchestrate helpers
- [ ] Test: ETA display during conversion

### Priority 2: `gui_updates.py:update_conversion_statistics` (159 lines)
- [ ] Extract `update_vmaf_display(gui, info)`
- [ ] Extract `update_crf_display(gui, info)`
- [ ] Extract `update_eta_display(gui, info)`
- [ ] Extract `update_size_prediction(gui, info)`
- [ ] Test: All stats update correctly during conversion

### Priority 3: `conversion_controller.py:force_stop_conversion` (134 lines)
- [ ] Extract `terminate_process(pid) -> bool`
- [ ] Extract `cleanup_temp_file(input_path)`
- [ ] Extract `restore_ui_after_stop(gui)`
- [ ] Test: Force stop button during conversion

### Priority 4: `conversion_controller.py:conversion_complete` (141 lines)
- [ ] Extract `build_summary_message(stats) -> str`
- [ ] Extract `format_error_details(errors) -> str`
- [ ] Extract `format_skip_details(skipped) -> str`
- [ ] Test: Completion summary displays correctly

---

## Phase 4: Linting & Type Hints
**Risk: Low | App remains functional: Yes**

### Tasks
- [ ] Run `uv run ruff check --fix src/` for auto-fixable issues
- [ ] Run `uv run ruff format src/`
- [ ] Add type hints to all new functions from Phase 3
- [ ] Fix `callable` → `Callable[...]` in function signatures (4 locations: `utils.py`, `video_conversion.py`, `ab_av1/wrapper.py`, `ab_av1/parser.py`)
- [ ] Fix `param: Type = None` → `param: Type | None = None` for optional parameters
- [ ] Replace bare `except:` with specific exception types (9 locations across `utils.py`, `ab_av1/wrapper.py`, `gui/main_window.py`)
- [ ] Run `uv run ty check src/` and fix errors
- [ ] Add `TypedDict` for any remaining dict parameters

---

## Phase 5: Linux/macOS Compatibility (Optional)
**Skip if Windows-only usage.**

- [ ] Fix process tree termination (`start_new_session=True` + `os.killpg()`)

---

## Not Doing

These were considered but rejected as over-engineering for a single-developer project:

- ❌ Full event bus / pub-sub system
- ❌ Dependency injection framework
- ❌ Abstract base classes / interfaces
- ❌ Separate `ConversionState` class (current pattern is acceptable)
- ❌ Thread locks (current `update_ui_safely` pattern is sufficient if used consistently)

---

## Execution Order

```
Phase 0 (Thread Safety)     <- Start here
Phase 1 (Data Models)
Phase 2 (Dependencies)
Phase 3 (Long Functions)
Phase 4 (Linting/Types)
Phase 5 (Linux/macOS)       <- Optional
```

## Minimum Viable Refactoring

If scope is limited, do only:
1. **Phase 0** - Thread safety (critical)
2. **Phase 3 Priority 1** - Extract `estimate_remaining_time` + dependencies (highest complexity)
3. **Phase 4** first two tasks - Auto-fix linting
