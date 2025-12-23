# src/gui/gui_updates.py
"""
GUI update functions (progress bars, statistics, labels, timers)
for the AV1 Video Converter application.
"""

# Standard library imports
import contextlib
import logging
import math  # For ceil
import statistics
import time

# GUI-related imports
import tkinter as tk  # For type hinting if needed

# Import constant from config
from src.config import DEFAULT_ENCODING_PRESET, DEFAULT_VMAF_TARGET
from src.estimation import estimate_remaining_time
from src.history_index import get_history_index
from src.models import ProgressEvent

# Project imports
from src.utils import (
    format_file_size,
    format_time,
    parse_eta_text,  # Add parse_eta_text import
    update_ui_safely,
)

logger = logging.getLogger(__name__)


def update_progress_bars(gui, quality_percent: float, encoding_percent: float) -> None:
    """Update the dual progress bars in a thread-safe way.

    Args:
        gui: The main GUI instance containing the progress bar widgets
        quality_percent: Percentage of quality detection progress (0-100)
        encoding_percent: Percentage of encoding progress (0-100)
    """

    def _update_ui():
        # Get GUI widgets
        quality_prog_widget = getattr(gui, "quality_progress", None)
        quality_label_widget = getattr(gui, "quality_percent_label", None)
        encoding_prog_widget = getattr(gui, "encoding_progress", None)
        encoding_label_widget = getattr(gui, "encoding_percent_label", None)

        # Make sure all widgets exist
        if not all([quality_prog_widget, quality_label_widget, encoding_prog_widget, encoding_label_widget]):
            return

        try:
            # Set mode for quality bar
            q_mode = "determinate"
            e_mode = "determinate"

            # Set quality progress to 100% when encoding is in progress
            if encoding_percent > 0:
                # When encoding has started, quality detection is complete
                display_quality_percent = 100
            else:
                # During quality detection phase
                display_quality_percent = quality_percent
                if quality_percent < 100 and encoding_percent <= 0:  # noqa: PLR2004
                    # Let's keep it determinate for now, as ab-av1 doesn't give specific phase progress
                    pass

            # Update the widgets (guarded by all() check above)
            quality_prog_widget.config(value=display_quality_percent, mode=q_mode)  # type: ignore[union-attr]
            quality_label_widget.config(text=f"{math.ceil(display_quality_percent)}%")  # type: ignore[union-attr]
            encoding_prog_widget.config(value=encoding_percent, mode=e_mode)  # type: ignore[union-attr]
            encoding_label_widget.config(text=f"{math.ceil(encoding_percent)}%")  # type: ignore[union-attr]

        except tk.TclError as e:
            logger.debug(f"TclError updating progress bars: {e}")

    update_ui_safely(gui.root, _update_ui)


def update_vmaf_display(gui, info: ProgressEvent) -> None:
    """Update the VMAF label based on conversion progress information.

    Args:
        gui: The main GUI instance containing the VMAF label widget
        info: ProgressEvent containing VMAF-related information
    """
    if info.vmaf is not None:
        try:
            vmaf_val = float(info.vmaf)
            vmaf_status = f"{vmaf_val:.1f}"
            if info.phase == "crf-search":
                vmaf_status += " (Current)"
            if info.used_fallback:
                vmaf_status += " (Fallback Used)"
            update_ui_safely(gui.root, lambda v=vmaf_status: gui.vmaf_label.config(text=v))
            logger.info(f"VMAF update: {vmaf_status}")
        except (ValueError, TypeError) as e:
            logger.warning(f"Invalid VMAF value in info for update: {info.vmaf} - {e}")
    elif info.phase == "crf-search":
        # Reset to target if in search phase without a specific VMAF value yet
        vmaf_target = info.vmaf_target_used if info.vmaf_target_used is not None else DEFAULT_VMAF_TARGET
        update_ui_safely(gui.root, lambda v=vmaf_target: gui.vmaf_label.config(text=f"{v} (Target)"))


def update_crf_display(gui, info: ProgressEvent) -> None:
    """Update the CRF and encoding settings label.

    Args:
        gui: The main GUI instance containing the encoding settings label
        info: ProgressEvent containing CRF-related information
    """
    if info.crf is not None:
        try:
            crf_val = int(info.crf)
            settings_text = f"CRF: {crf_val}, Preset: {DEFAULT_ENCODING_PRESET}"
            update_ui_safely(gui.root, lambda s=settings_text: gui.encoding_settings_label.config(text=s))
            logger.info(f"Encoding settings update: {settings_text}")
        except (ValueError, TypeError) as e:
            logger.warning(f"Invalid CRF value in info for update: {info.crf} - {e}")


def update_eta_from_progress_info(gui, info: ProgressEvent) -> None:
    """Update the ETA (Estimated Time of Arrival) label from progress info.

    Args:
        gui: The main GUI instance containing the ETA label
        info: ProgressEvent containing progress and timing information
    """
    encoding_prog = info.progress_encoding
    gui.session.last_encoding_progress = encoding_prog
    logger.debug(f"Encoding progress: {encoding_prog}, Phase: {info.phase}")

    # Check if we have AB-AV1's ETA text
    if info.eta_text:
        eta_seconds = parse_eta_text(info.eta_text)
        if eta_seconds > 0 or gui.session.last_eta_seconds is None:
            gui.session.last_eta_seconds = eta_seconds
            gui.session.last_eta_timestamp = time.time()
            logger.debug(f"Captured AB-AV1 ETA: {info.eta_text} -> {eta_seconds} seconds")

    if encoding_prog > 0:
        # Mark when encoding phase starts
        if (
            gui.session.current_file_start_time
            and not gui.session.current_file_encoding_start_time
            and info.phase == "encoding"
        ):
            gui.session.current_file_encoding_start_time = time.time()
            logger.info("Encoding phase started - initializing timer")

        # Use the stored AB-AV1 ETA and count down
        if gui.session.last_eta_seconds is not None and gui.session.last_eta_timestamp is not None:
            elapsed_since_update = time.time() - gui.session.last_eta_timestamp
            remaining_eta = max(0, gui.session.last_eta_seconds - elapsed_since_update)
            eta_str = format_time(remaining_eta)
            update_ui_safely(gui.root, lambda eta=eta_str: gui.eta_label.config(text=eta))
            logger.debug(
                f"ETA countdown: {eta_str} (base: {gui.session.last_eta_seconds}s, "
                f"elapsed: {elapsed_since_update:.1f}s)"
            )
        elif gui.session.current_file_encoding_start_time:
            # Fallback: calculate based on progress if no AB-AV1 ETA is stored
            elapsed_encoding_time = time.time() - gui.session.current_file_encoding_start_time
            if encoding_prog > 0 and elapsed_encoding_time > 1:
                try:
                    total_encoding_time_est = (elapsed_encoding_time / encoding_prog) * 100
                    eta_seconds = total_encoding_time_est - elapsed_encoding_time
                    eta_str = format_time(eta_seconds)
                    update_ui_safely(gui.root, lambda eta=eta_str: gui.eta_label.config(text=eta))
                    logger.debug(
                        f"ETA calculation fallback: {eta_str} "
                        f"(progress: {encoding_prog:.1f}%, elapsed: {format_time(elapsed_encoding_time)})"
                    )
                except ZeroDivisionError:
                    update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Calculating..."))
                except Exception:
                    logger.exception("Error calculating ETA")
                    update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Error"))
            else:
                update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Calculating..."))
        else:
            update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Calculating..."))
    elif info.phase == "crf-search":
        update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Detecting..."))
    else:
        update_ui_safely(gui.root, lambda: gui.eta_label.config(text="-"))


def update_size_prediction(gui, info: ProgressEvent) -> None:
    """Update the output size prediction label.

    Args:
        gui: The main GUI instance containing the output size label
        info: ProgressEvent containing size-related information
    """
    # Check if we have an estimated_output_size from the partial file
    if info.output_size is not None and info.original_size is not None and info.is_estimate:
        current_size = info.output_size
        original_size = info.original_size
        if original_size > 0:
            try:
                current_size_f = float(current_size)
                original_size_f = float(original_size)
                ratio = (current_size_f / original_size_f) * 100
                size_str = f"{format_file_size(int(current_size_f))} ({ratio:.1f}%) [Est]"
                update_ui_safely(gui.root, lambda s=size_str: gui.output_size_label.config(text=s))
                logger.info(f"Output size estimate (from partial file): {size_str}")
            except (ValueError, TypeError, ZeroDivisionError) as e:
                logger.warning(f"Invalid size data for estimate: {current_size}, {original_size} - {e}")
    # Check if we have a size_reduction percentage
    elif info.size_reduction is not None:
        try:
            if gui.session.last_input_size:
                original_size = gui.session.last_input_size
                size_reduction_f = float(info.size_reduction)
                size_percentage = 100.0 - size_reduction_f
                output_size_estimate = original_size * (size_percentage / 100.0)
                ratio = size_percentage
                size_str = f"{format_file_size(int(output_size_estimate))} ({ratio:.1f}%)"
                gui.session.last_output_size = output_size_estimate
                update_ui_safely(gui.root, lambda s=size_str: gui.output_size_label.config(text=s))
                logger.info(f"Output size prediction: {size_str} (reduction: {info.size_reduction:.1f}%)")
        except (ValueError, TypeError, ZeroDivisionError):
            logger.exception("Error calculating output size from reduction")
        except Exception:
            logger.exception("Unexpected error calculating output size from reduction")
    # Direct size information if available (typically on completion)
    elif info.output_size is not None and info.original_size is not None:
        current_size = info.output_size
        original_size = info.original_size
        if original_size > 0:
            try:
                current_size_f = float(current_size)
                original_size_f = float(original_size)
                ratio = (current_size_f / original_size_f) * 100
                size_str = f"{format_file_size(int(current_size_f))} ({ratio:.1f}%)"
                update_ui_safely(gui.root, lambda s=size_str: gui.output_size_label.config(text=s))
                logger.info(f"Output size update: {size_str}")
            except (ValueError, TypeError, ZeroDivisionError) as e:
                logger.warning(f"Invalid size data for final update: {current_size}, {original_size} - {e}")


def update_conversion_statistics(gui, info: ProgressEvent | None = None) -> None:
    """Update the conversion statistics like ETA, VMAF, CRF in the UI.

    Args:
        gui: The main GUI instance containing statistic display widgets
        info: ProgressEvent containing conversion progress information
    """
    if not info or not gui.session.running:
        logger.debug("Skipping update_conversion_statistics: no info or not running")
        return

    # Log received info for debugging
    logger.debug(f"Processing statistics update: {info}")

    # Update individual display components
    update_vmaf_display(gui, info)
    update_crf_display(gui, info)
    update_eta_from_progress_info(gui, info)
    update_size_prediction(gui, info)


def update_elapsed_time(gui, start_time: float) -> None:
    """Update the elapsed time label for current file only.

    Args:
        gui: The main GUI instance containing the elapsed time label
        start_time: Timestamp when processing of the current file started
    """
    if not gui.session.running or (gui.stop_event and gui.stop_event.is_set()):
        gui.session.elapsed_timer_id = None
        return

    # During encoding phase, show time since encoding started
    if gui.session.current_file_encoding_start_time:
        current_file_elapsed = time.time() - gui.session.current_file_encoding_start_time
    else:
        # During quality detection phase, show the full time
        current_file_elapsed = time.time() - start_time

    update_ui_safely(gui.root, lambda t=current_file_elapsed: gui.elapsed_label.config(text=format_time(t)))

    # Also update total elapsed time
    update_total_elapsed_time(gui)

    # Update ETA countdown if we're in encoding phase
    update_eta_countdown(gui)

    # Remove direct call to update_total_remaining_time since it's now on its own timer

    # Schedule next update
    gui.session.elapsed_timer_id = gui.root.after(1000, lambda: update_elapsed_time(gui, start_time))


def update_total_elapsed_time(gui) -> None:
    """Update the total elapsed time label for the entire conversion batch.

    Args:
        gui: The main GUI instance containing the total elapsed time label
    """
    if gui.session.total_start_time and gui.session.running:
        total_elapsed = time.time() - gui.session.total_start_time
        update_ui_safely(gui.root, lambda t=total_elapsed: gui.total_elapsed_label.config(text=format_time(t)))
    else:
        update_ui_safely(gui.root, lambda: gui.total_elapsed_label.config(text="-"))


def update_total_remaining_time(gui) -> None:
    """Update the total estimated remaining time label.

    Args:
        gui: The main GUI instance containing the total remaining time label
    """
    if not gui.session.running:
        update_ui_safely(gui.root, lambda: gui.total_remaining_label.config(text="-"))
        return

    try:
        # Estimate remaining time
        total_remaining = estimate_remaining_time(gui)
        remaining_str = format_time(total_remaining) if total_remaining > 0 else "Calculating..."

        update_ui_safely(gui.root, lambda r=remaining_str: gui.total_remaining_label.config(text=r))
    except Exception:
        logger.exception("Error updating total remaining time")
        update_ui_safely(gui.root, lambda: gui.total_remaining_label.config(text="Error"))


def update_statistics_summary(gui) -> None:
    """Update the overall statistics summary labels with historical data from conversion_history_v2.json.

    Args:
        gui: The main GUI instance containing statistic labels
    """
    vmaf_avg_text = "-"
    vmaf_range_text = ""
    crf_avg_text = "-"
    crf_range_text = ""
    reduction_avg_text = "-"
    reduction_range_text = ""
    total_saved_text = "-"

    # Load and process historical data from index
    try:
        index = get_history_index()
        converted_records = index.get_converted_records()

        if converted_records:
            # Extract statistics from all historical conversions
            historical_vmaf = []
            historical_crf = []
            historical_reductions = []
            total_input_size = 0
            total_output_size = 0
            total_conversions = 0

            for record in converted_records:
                # VMAF
                if record.final_vmaf is not None:
                    with contextlib.suppress(ValueError, TypeError):
                        historical_vmaf.append(float(record.final_vmaf))

                # CRF
                if record.final_crf is not None:
                    with contextlib.suppress(ValueError, TypeError):
                        historical_crf.append(int(record.final_crf))

                # Size reduction
                if record.reduction_percent is not None:
                    with contextlib.suppress(ValueError, TypeError):
                        historical_reductions.append(float(record.reduction_percent))

                # Accumulate total sizes (now in bytes, not MB)
                if record.file_size_bytes:
                    with contextlib.suppress(ValueError, TypeError):
                        total_input_size += record.file_size_bytes

                if record.output_size_bytes:
                    with contextlib.suppress(ValueError, TypeError):
                        total_output_size += record.output_size_bytes

                total_conversions += 1

            # Calculate statistics from historical data
            if historical_vmaf:
                try:
                    avg_vmaf = statistics.mean(historical_vmaf)
                    min_vmaf = min(historical_vmaf)
                    max_vmaf = max(historical_vmaf)
                    vmaf_avg_text = f"Avg: {avg_vmaf:.1f}"
                    vmaf_range_text = f"(Range: {min_vmaf:.1f}-{max_vmaf:.1f})"
                    logger.debug(f"Historical VMAF stats: avg={avg_vmaf:.1f}, min={min_vmaf:.1f}, max={max_vmaf:.1f}")
                except statistics.StatisticsError:
                    if len(historical_vmaf) == 1:
                        vmaf_avg_text = f"{historical_vmaf[0]:.1f}"
                        vmaf_range_text = ""
                    else:
                        vmaf_avg_text = "Error"
                        vmaf_range_text = ""
                except Exception as e:
                    logger.warning(f"Error calculating historical VMAF stats: {e}")
                    vmaf_avg_text = "Error"
                    vmaf_range_text = ""

            if historical_crf:
                try:
                    avg_crf = statistics.mean(historical_crf)
                    min_crf = min(historical_crf)
                    max_crf = max(historical_crf)
                    crf_avg_text = f"Avg: {avg_crf:.1f}"
                    crf_range_text = f"(Range: {min_crf}-{max_crf})"
                    logger.debug(f"Historical CRF stats: avg={avg_crf:.1f}, min={min_crf}, max={max_crf}")
                except statistics.StatisticsError:
                    if len(historical_crf) == 1:
                        crf_avg_text = f"{historical_crf[0]}"
                        crf_range_text = ""
                    else:
                        crf_avg_text = "Error"
                        crf_range_text = ""
                except Exception as e:
                    logger.warning(f"Error calculating historical CRF stats: {e}")
                    crf_avg_text = "Error"
                    crf_range_text = ""

            if historical_reductions:
                try:
                    avg_reduction = statistics.mean(historical_reductions)
                    min_reduction = min(historical_reductions)
                    max_reduction = max(historical_reductions)
                    reduction_avg_text = f"Avg: {avg_reduction:.1f}%"
                    reduction_range_text = f"(Range: {min_reduction:.1f}%-{max_reduction:.1f}%)"
                    logger.info(
                        f"Historical Size reduction stats: avg={avg_reduction:.1f}%, "
                        f"min={min_reduction:.1f}%, max={max_reduction:.1f}%"
                    )
                except statistics.StatisticsError:
                    if len(historical_reductions) == 1:
                        reduction_avg_text = f"{historical_reductions[0]:.1f}%"
                        reduction_range_text = ""
                    else:
                        reduction_avg_text = "Error"
                        reduction_range_text = ""
                except Exception as e:
                    logger.warning(f"Error calculating historical Size Reduction stats: {e}")
                    reduction_avg_text = "Error"
                    reduction_range_text = ""

            # Add total space saved and conversion count
            if total_input_size > 0 and total_output_size > 0:
                total_saved = total_input_size - total_output_size
                total_saved_text = f"{format_file_size(int(total_saved))} ({total_conversions} files)"

        # No conversion history - show current session stats instead
        elif gui.session.running or gui.session.processed_files > 0:
            if gui.session.vmaf_scores:
                try:
                    avg_vmaf = statistics.mean(gui.session.vmaf_scores)
                    min_vmaf = min(gui.session.vmaf_scores)
                    max_vmaf = max(gui.session.vmaf_scores)
                    vmaf_avg_text = f"Avg: {avg_vmaf:.1f}"
                    vmaf_range_text = f"(Range: {min_vmaf:.1f}-{max_vmaf:.1f})"
                except (statistics.StatisticsError, Exception):
                    if len(gui.session.vmaf_scores) == 1:
                        vmaf_avg_text = f"{gui.session.vmaf_scores[0]:.1f}"
                        vmaf_range_text = ""
                    else:
                        vmaf_avg_text = "Error"
                        vmaf_range_text = ""

            if gui.session.crf_values:
                try:
                    avg_crf = statistics.mean(gui.session.crf_values)
                    min_crf = min(gui.session.crf_values)
                    max_crf = max(gui.session.crf_values)
                    crf_avg_text = f"Avg: {avg_crf:.1f}"
                    crf_range_text = f"(Range: {min_crf}-{max_crf})"
                except (statistics.StatisticsError, Exception):
                    if len(gui.session.crf_values) == 1:
                        crf_avg_text = f"{gui.session.crf_values[0]}"
                        crf_range_text = ""
                    else:
                        crf_avg_text = "Error"
                        crf_range_text = ""

            if gui.session.size_reductions:
                try:
                    valid_reductions = [r for r in gui.session.size_reductions if isinstance(r, (int, float))]
                    if valid_reductions:
                        avg_reduction = statistics.mean(valid_reductions)
                        min_reduction = min(valid_reductions)
                        max_reduction = max(valid_reductions)
                        reduction_avg_text = f"Avg: {avg_reduction:.1f}%"
                        reduction_range_text = f"(Range: {min_reduction:.1f}%-{max_reduction:.1f}%)"
                    else:
                        reduction_avg_text = "No data"
                        reduction_range_text = ""
                except (statistics.StatisticsError, Exception):
                    if len(valid_reductions) == 1:
                        reduction_avg_text = f"{valid_reductions[0]:.1f}%"
                        reduction_range_text = ""
                    else:
                        reduction_avg_text = "Error"
                        reduction_range_text = ""
    except Exception:
        logger.exception("Error loading historical statistics")
        # Fallback to current session stats
        return update_statistics_summary_current_session(gui)

    # Use lambdas with explicit parameter capturing for UI updates
    update_ui_safely(gui.root, lambda v=vmaf_avg_text: gui.vmaf_stats_label.config(text=v))
    update_ui_safely(gui.root, lambda v=vmaf_range_text: gui.vmaf_range_label.config(text=v))
    update_ui_safely(gui.root, lambda c=crf_avg_text: gui.crf_stats_label.config(text=c))
    update_ui_safely(gui.root, lambda c=crf_range_text: gui.crf_range_label.config(text=c))
    update_ui_safely(gui.root, lambda r=reduction_avg_text: gui.size_stats_label.config(text=r))
    update_ui_safely(gui.root, lambda r=reduction_range_text: gui.size_range_label.config(text=r))
    update_ui_safely(gui.root, lambda t=total_saved_text: gui.total_saved_label.config(text=t))
    return None


def update_statistics_summary_current_session(gui) -> None:
    """Update the overall statistics summary labels with current session data.

    Args:
        gui: The main GUI instance containing statistic labels and data
    """
    vmaf_avg_text = "-"
    vmaf_range_text = ""
    crf_avg_text = "-"
    crf_range_text = ""
    reduction_avg_text = "-"
    reduction_range_text = ""
    total_saved_text = "-"

    # Debug logging to trace updates
    logger.debug(
        f"Updating statistics summary - VMAF scores: {len(gui.session.vmaf_scores)}, "
        f"CRF values: {len(gui.session.crf_values)}, Size reductions: {len(gui.session.size_reductions)}"
    )

    if gui.session.vmaf_scores:
        try:
            avg_vmaf = statistics.mean(gui.session.vmaf_scores)
            min_vmaf = min(gui.session.vmaf_scores)
            max_vmaf = max(gui.session.vmaf_scores)
            vmaf_avg_text = f"Avg: {avg_vmaf:.1f}"
            vmaf_range_text = f"(Range: {min_vmaf:.1f}-{max_vmaf:.1f})"
            logger.debug(f"VMAF stats: avg={avg_vmaf:.1f}, min={min_vmaf:.1f}, max={max_vmaf:.1f}")
        except statistics.StatisticsError:  # Handle case with insufficient data
            if len(gui.session.vmaf_scores) == 1:
                vmaf_avg_text = f"{gui.session.vmaf_scores[0]:.1f}"
                vmaf_range_text = ""
            else:
                vmaf_avg_text = "Error"
                vmaf_range_text = ""
                logger.warning("StatisticsError calculating VMAF stats")
        except Exception as e:
            logger.warning(f"Error calculating VMAF stats: {e}")
            vmaf_avg_text = "Error"
            vmaf_range_text = ""

    if gui.session.crf_values:
        try:
            avg_crf = statistics.mean(gui.session.crf_values)
            min_crf = min(gui.session.crf_values)
            max_crf = max(gui.session.crf_values)
            crf_avg_text = f"Avg: {avg_crf:.1f}"
            crf_range_text = f"(Range: {min_crf}-{max_crf})"
            logger.debug(f"CRF stats: avg={avg_crf:.1f}, min={min_crf}, max={max_crf}")
        except statistics.StatisticsError:  # Handle case with insufficient data
            if len(gui.session.crf_values) == 1:
                crf_avg_text = f"{gui.session.crf_values[0]}"
                crf_range_text = ""
            else:
                crf_avg_text = "Error"
                crf_range_text = ""
                logger.warning("StatisticsError calculating CRF stats")
        except Exception as e:
            logger.warning(f"Error calculating CRF stats: {e}")
            crf_avg_text = "Error"
            crf_range_text = ""

    if gui.session.size_reductions:
        try:
            # Make sure the list isn't empty and contains valid numbers
            valid_reductions = [r for r in gui.session.size_reductions if isinstance(r, (int, float))]
            if not valid_reductions:
                reduction_avg_text = "No data"
                reduction_range_text = ""
            else:
                avg_reduction = statistics.mean(valid_reductions)
                min_reduction = min(valid_reductions)
                max_reduction = max(valid_reductions)
                reduction_avg_text = f"Avg: {avg_reduction:.1f}%"
                reduction_range_text = f"(Range: {min_reduction:.1f}%-{max_reduction:.1f}%)"
                logger.info(
                    f"Size reduction stats: avg={avg_reduction:.1f}%, "
                    f"min={min_reduction:.1f}%, max={max_reduction:.1f}%"
                )
        except statistics.StatisticsError:  # Handle case with insufficient data
            if len(valid_reductions) == 1:
                reduction_avg_text = f"{valid_reductions[0]:.1f}%"
                reduction_range_text = ""
            else:
                reduction_avg_text = "Error"
                reduction_range_text = ""
                logger.warning("StatisticsError calculating Size Reduction stats")
        except Exception as e:
            logger.warning(f"Error calculating Size Reduction stats: {e}")
            logger.warning(f"Size reduction values: {gui.session.size_reductions}")
            reduction_avg_text = "Error"
            reduction_range_text = ""

    # Calculate total saved for current session
    if gui.session.total_input_bytes_success > 0 and gui.session.total_output_bytes_success > 0:
        total_saved = gui.session.total_input_bytes_success - gui.session.total_output_bytes_success
        if total_saved > 0:
            total_saved_text = f"{format_file_size(total_saved)} ({gui.session.successful_conversions} files)"

    # Use lambdas with explicit parameter capturing for UI updates
    update_ui_safely(gui.root, lambda v=vmaf_avg_text: gui.vmaf_stats_label.config(text=v))
    update_ui_safely(gui.root, lambda v=vmaf_range_text: gui.vmaf_range_label.config(text=v))
    update_ui_safely(gui.root, lambda c=crf_avg_text: gui.crf_stats_label.config(text=c))
    update_ui_safely(gui.root, lambda c=crf_range_text: gui.crf_range_label.config(text=c))
    update_ui_safely(gui.root, lambda r=reduction_avg_text: gui.size_stats_label.config(text=r))
    update_ui_safely(gui.root, lambda r=reduction_range_text: gui.size_range_label.config(text=r))
    update_ui_safely(gui.root, lambda t=total_saved_text: gui.total_saved_label.config(text=t))


def update_eta_countdown(gui) -> None:
    """Update the ETA display by counting down from stored AB-AV1 ETA.
    This function is called every second to provide continuous countdown.
    """
    if not gui.session.running:
        return

    # Use the stored AB-AV1 ETA and count down
    if gui.session.last_eta_seconds is not None and gui.session.last_eta_timestamp is not None:
        elapsed_since_update = time.time() - gui.session.last_eta_timestamp
        remaining_eta = max(0, gui.session.last_eta_seconds - elapsed_since_update)
        eta_str = format_time(remaining_eta)
        update_ui_safely(gui.root, lambda eta=eta_str: gui.eta_label.config(text=eta))
        return

    # Fallback to calculation based on progress
    encoding_prog = gui.session.last_encoding_progress

    if encoding_prog > 0 and gui.session.current_file_encoding_start_time:
        elapsed_encoding_time = time.time() - gui.session.current_file_encoding_start_time
        if encoding_prog > 0 and elapsed_encoding_time > 1:
            try:
                total_encoding_time_est = (elapsed_encoding_time / encoding_prog) * 100
                eta_seconds = total_encoding_time_est - elapsed_encoding_time
                eta_str = format_time(eta_seconds)
                update_ui_safely(gui.root, lambda eta=eta_str: gui.eta_label.config(text=eta))
            except ZeroDivisionError:
                pass
            except Exception:
                logger.exception("Error updating ETA display")


def reset_current_file_details(gui) -> None:
    """Reset labels related to the currently processing file to default values.

    Args:
        gui: The main GUI instance containing file detail UI elements
    """
    # Use constant imported from src.config
    default_vmaf_text = f"{DEFAULT_VMAF_TARGET} (Target)"
    update_ui_safely(gui.root, lambda: gui.current_file_label.config(text="No file processing"))
    update_ui_safely(gui.root, update_progress_bars, gui, 0, 0)  # Use helper
    update_ui_safely(gui.root, lambda: gui.orig_format_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.orig_size_label.config(text="-"))
    update_ui_safely(gui.root, lambda v=default_vmaf_text: gui.vmaf_label.config(text=v))
    update_ui_safely(gui.root, lambda: gui.elapsed_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.eta_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.output_size_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.encoding_settings_label.config(text="-"))
    gui.session.current_file_encoding_start_time = None
    gui.session.last_encoding_progress = 0.0  # Reset last progress for ETA calculation
    # Reset stored AB-AV1 ETA values
    gui.session.last_eta_seconds = None
    gui.session.last_eta_timestamp = None
