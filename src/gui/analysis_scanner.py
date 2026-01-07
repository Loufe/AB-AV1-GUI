# src/gui/analysis_scanner.py
"""
Analysis scanner module - extracted from main_window.py.

Provides incremental folder scanning and ffprobe analysis for the Analysis tab.
"""

import logging
import os
import threading
from collections import deque
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from pathlib import Path

from src.cache_helpers import mtimes_match
from src.config import MIN_FILES_FOR_PERCENT_UPDATES, TREE_UPDATE_BATCH_SIZE
from src.estimation import compute_grouped_percentiles
from src.folder_analysis import _analyze_file
from src.gui.tree_display import compute_analysis_display_values
from src.history_index import get_history_index
from src.models import FileStatus
from src.utils import format_file_size, update_ui_safely

logger = logging.getLogger(__name__)


def incremental_scan_thread(gui, folder: str, extensions: list[str], stop_event: threading.Event):
    """Scan folder and populate tree incrementally from background thread.

    Uses depth-first traversal for optimal HDD performance - keeps disk head
    in the same directory subtree, minimizing seek times.

    Also checks HistoryIndex cache - if a file was previously analyzed,
    displays cached values immediately instead of "—".
    """
    root_folder = str(Path(folder).resolve())
    ext_set = {f".{ext.lower()}" for ext in extensions}
    file_count = 0
    folder_count = 0
    index = get_history_index()

    def scan_directory(dirpath: str) -> tuple[list[str], list[tuple[str, int, float]]]:
        """Scan a directory for subdirs and video files with stats.

        Returns:
            (subdirs, file_infos) where file_infos is list of (filename, size, mtime)
        """
        subdirs = []
        file_infos = []
        try:
            with os.scandir(dirpath) as entries:
                for entry in entries:
                    if stop_event.is_set():
                        return [], []
                    if entry.is_dir(follow_symlinks=False):
                        subdirs.append(entry.path)
                    elif entry.is_file() and os.path.splitext(entry.name)[1].lower() in ext_set:
                        try:
                            stat = entry.stat()
                            file_infos.append((entry.name, stat.st_size, stat.st_mtime))
                        except OSError:
                            file_infos.append((entry.name, 0, 0))
        except (PermissionError, OSError):
            pass
        return sorted(subdirs, key=str.lower), sorted(file_infos, key=lambda x: x[0].lower())

    try:
        # DFS stack: (dirpath, parent_dirpath or None for root)
        stack: deque[tuple[str, str | None]] = deque()
        stack.append((root_folder, None))

        # Track folder tree IDs - populated by UI callbacks
        folder_tree_ids: dict[str, str] = {}
        folder_tree_ids[root_folder] = ""  # Root maps to tree root

        # Pre-compute percentiles once for entire scan (history doesn't change during scan)
        grouped_percentiles = compute_grouped_percentiles()

        while stack and not stop_event.is_set():
            dirpath, parent_dirpath = stack.pop()

            # Scan directory in background thread
            subdirs, file_infos = scan_directory(dirpath)
            if stop_event.is_set():
                break

            # Get parent tree ID
            parent_tree_id = folder_tree_ids.get(parent_dirpath or root_folder, "")

            # Push subdirectories for DFS (reversed so alphabetical order pops first)
            for subdir in reversed(subdirs):
                stack.append((subdir, dirpath))

            # Pre-compute cached values for each file (in background thread)
            # This avoids doing index lookups on the UI thread
            file_display_data = []
            for filename, file_size, file_mtime in file_infos:
                file_path = os.path.join(dirpath, filename)

                # Check cache (use tolerance for mtime due to float precision in JSON)
                record = index.lookup_file(file_path)
                if record and record.file_size_bytes == file_size and mtimes_match(record.file_mtime, file_mtime):
                    # ADR-001: Check for better duplicate if status is SCANNED/ANALYZED
                    if record.status in (FileStatus.SCANNED, FileStatus.ANALYZED):
                        better = index.find_better_duplicate(file_path, record.file_size_bytes, record.duration_sec)
                        if better:
                            record = better
                    # Cache hit - use cached display values
                    format_str, size_str, savings_str, time_str, eff_str, tag = compute_analysis_display_values(
                        record, grouped_percentiles=grouped_percentiles
                    )
                else:
                    # No valid cache - show defaults until scanned
                    format_str = "—"
                    size_str = format_file_size(file_size)
                    savings_str = "—"
                    time_str = "—"
                    eff_str = "—"
                    tag = ""

                file_display_data.append(
                    (filename, file_path, format_str, size_str, savings_str, time_str, eff_str, tag)
                )

            # Prepare UI update
            is_root = dirpath == root_folder
            folder_name = os.path.basename(dirpath) if not is_root else None

            # Use event to wait for UI update to complete
            done_event = threading.Event()
            new_folder_id: list[str] = [""]  # Mutable container to get result back

            def add_to_tree(
                dp=dirpath,
                pid=parent_tree_id,
                fname=folder_name,
                fdata=file_display_data,
                is_rt=is_root,
                result=new_folder_id,
                done=done_event,
            ):
                nonlocal file_count, folder_count
                try:
                    if is_rt:
                        # Root folder: add files at tree root, no folder node
                        folder_id = ""
                        for filename, file_path, format_str, size_str, savings_str, time_str, eff_str, tag in fdata:
                            item_id = gui.analysis_tree.insert(
                                "",
                                "end",
                                text=f"🎬 {filename}",
                                values=(format_str, size_str, savings_str, time_str, eff_str),
                                tags=(tag,) if tag else (),
                            )
                            gui.get_tree_item_map()[os.path.normcase(file_path)] = item_id
                            file_count += 1
                    else:
                        # Non-root: create folder node and add files
                        folder_id = gui.analysis_tree.insert(
                            pid, "end", text=f"▶ 📁 {fname}", values=("", "—", "—", "—", "—"), open=False
                        )
                        folder_count += 1
                        for filename, file_path, format_str, size_str, savings_str, time_str, eff_str, tag in fdata:
                            item_id = gui.analysis_tree.insert(
                                folder_id,
                                "end",
                                text=f"🎬 {filename}",
                                values=(format_str, size_str, savings_str, time_str, eff_str),
                                tags=(tag,) if tag else (),
                            )
                            gui.get_tree_item_map()[os.path.normcase(file_path)] = item_id
                            file_count += 1
                        # Update folder aggregate from its files
                        if fdata:
                            gui.update_folder_aggregates(folder_id)

                    # Update scanning badge with current file count
                    file_word = "file" if file_count == 1 else "files"
                    gui.analysis_scan_badge.config(text=f"Scanning... ({file_count} {file_word})")

                    result[0] = folder_id
                finally:
                    done.set()

            update_ui_safely(gui.root, add_to_tree)
            done_event.wait(timeout=5.0)  # Wait for UI thread

            # Store folder ID for children to use
            folder_tree_ids[dirpath] = new_folder_id[0]

        # Final status
        if stop_event.is_set():
            update_ui_safely(gui.root, lambda: gui.finish_incremental_scan(stopped=True))
            return

        update_ui_safely(gui.root, lambda: gui.finish_incremental_scan(stopped=False))

    except PermissionError:
        logger.exception("Permission denied during scan")
        if not stop_event.is_set():
            update_ui_safely(gui.root, lambda: gui.finish_incremental_scan(stopped=False))
    except OSError:
        logger.exception("OS error during scan")
        if not stop_event.is_set():
            update_ui_safely(gui.root, lambda: gui.finish_incremental_scan(stopped=False))
    except Exception:
        logger.exception("Error during incremental scan")
        if not stop_event.is_set():
            update_ui_safely(gui.root, lambda: gui.finish_incremental_scan(stopped=False))


def run_ffprobe_analysis(gui, file_paths: list[str], output_folder: str, input_folder: str, anonymize: bool):
    """Run ffprobe analysis on files in parallel.

    This analyzes files already in the tree using ffprobe to get metadata
    and estimate potential savings. Updates tree rows as results come in.

    Files with valid cache entries return quickly (no ffprobe needed).
    Cache checking is done inside each parallel worker, so there's no
    blocking pre-filter step.

    Args:
        file_paths: List of file paths to analyze.
        output_folder: Output folder for checking if files are already converted.
        input_folder: Input folder path (captured from main thread).
        anonymize: Whether to anonymize history (captured from main thread).
    """
    index = get_history_index()
    root_path = Path(input_folder).resolve()
    output_path = Path(output_folder).resolve()

    total_files = len(file_paths)
    files_completed = 0
    cache_hits = 0
    max_workers = min(8, max(4, total_files // 10 + 1))

    def analyze_one_file(file_path: str) -> tuple[str | None, bool]:
        """Analyze a single file (runs in thread pool).

        Checks cache first - if valid, skips ffprobe.

        Returns:
            Tuple of (file_path or None, was_cache_hit).
        """
        if gui.analysis_stop_event and gui.analysis_stop_event.is_set():
            return None, False

        # Check cache first - if valid, skip ffprobe
        try:
            stat = os.stat(file_path)
            cached = index.lookup_file(file_path)
            if cached and cached.file_size_bytes == stat.st_size and mtimes_match(cached.file_mtime, stat.st_mtime):
                return file_path, True  # Cache hit - no ffprobe needed
        except OSError:
            pass  # Let _analyze_file handle the error

        # Cache miss - run full analysis with ffprobe
        try:
            _analyze_file(file_path, root_path, output_path, index, anonymize)
            return file_path, False
        except Exception:
            logger.exception(f"Error analyzing {os.path.basename(file_path)}")
            return None, False

    try:
        with ThreadPoolExecutor(max_workers=max_workers) as executor:
            futures = {executor.submit(analyze_one_file, fp): fp for fp in file_paths}
            pending = set(futures.keys())

            while pending:
                # Check stop event before waiting for futures
                if gui.analysis_stop_event and gui.analysis_stop_event.is_set():
                    logger.info("Analysis interrupted by user")
                    executor.shutdown(wait=True, cancel_futures=True)
                    index.save()
                    return  # finally block will call on_ffprobe_complete

                # Wait for futures with timeout to allow periodic stop checks
                done, pending = wait(pending, timeout=0.5, return_when=FIRST_COMPLETED)

                # Collect completed files for batch UI update
                completed_paths: list[str] = []

                for future in done:
                    file_path, was_cached = future.result()
                    files_completed += 1
                    if was_cached:
                        cache_hits += 1
                    if file_path:
                        completed_paths.append(file_path)

                # Single batched UI update for all completed files in this round
                if completed_paths:
                    paths_snapshot = list(completed_paths)
                    update_ui_safely(gui.root, lambda paths=paths_snapshot: gui.batch_update_tree_rows(paths))

                # Update progress badge
                pct = int(100 * files_completed / total_files)
                text = f"Analyzing {pct}% ({files_completed}/{total_files} files)"
                update_ui_safely(gui.root, lambda t=text: gui.analysis_scan_badge.config(text=t))

                # Update totals and save less frequently (every batch or 5% progress)
                batch_interval = TREE_UPDATE_BATCH_SIZE
                pct_interval = max(1, total_files // 20)  # 5% increments
                if files_completed % batch_interval == 0 or (
                    total_files > MIN_FILES_FOR_PERCENT_UPDATES and files_completed % pct_interval == 0
                ):
                    update_ui_safely(gui.root, gui.update_total_from_tree)
                    index.save()

        # Save index after all files processed (handles remainder)
        index.save()

        # Log cache efficiency
        if cache_hits > 0:
            logger.info(f"Analysis complete: {cache_hits}/{total_files} from cache")
    except Exception:
        logger.exception("Unexpected error during ffprobe analysis")
    finally:
        # Always cleanup UI state (hide badge, re-enable buttons)
        update_ui_safely(gui.root, gui.on_ffprobe_complete)
