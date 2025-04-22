# src/gui/gui_updates.py
"""
GUI update functions (progress bars, statistics, labels, timers)
for the AV1 Video Converter application.
"""
# Standard library imports
import time
import logging
import statistics
import math # For ceil

# GUI-related imports
import tkinter as tk # For type hinting if needed

# Project imports
from src.utils import (
    format_time, format_file_size, update_ui_safely, load_history, estimate_remaining_time,
    parse_eta_text  # Add parse_eta_text import
)
import src.utils as utils  # For the new estimation functions
# Import constant from config
from src.config import DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET

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
        quality_prog_widget = getattr(gui, 'quality_progress', None)
        quality_label_widget = getattr(gui, 'quality_percent_label', None)
        encoding_prog_widget = getattr(gui, 'encoding_progress', None)
        encoding_label_widget = getattr(gui, 'encoding_percent_label', None)

        # Make sure all widgets exist
        if not all([quality_prog_widget, quality_label_widget, encoding_prog_widget, encoding_label_widget]):
            return

        try:
            # Set mode for quality bar
            q_mode = 'determinate'
            e_mode = 'determinate'

            # Set quality progress to 100% when encoding is in progress
            if encoding_percent > 0:
                # When encoding has started, quality detection is complete
                display_quality_percent = 100
            else:
                # During quality detection phase
                display_quality_percent = quality_percent
                if quality_percent < 100 and encoding_percent <= 0:
                    # Let's keep it determinate for now, as ab-av1 doesn't give specific phase progress
                    # q_mode = 'indeterminate' # Consider this if phase detection is more granular
                    pass

            # Update the widgets
            quality_prog_widget.config(value=display_quality_percent, mode=q_mode)
            quality_label_widget.config(text=f"{math.ceil(display_quality_percent)}%")
            encoding_prog_widget.config(value=encoding_percent, mode=e_mode)
            encoding_label_widget.config(text=f"{math.ceil(encoding_percent)}%")

        except tk.TclError as e:
            logger.debug(f"TclError updating progress bars: {e}")

    update_ui_safely(gui.root, _update_ui)


def update_conversion_statistics(gui, info: dict = None) -> None:
    """Update the conversion statistics like ETA, VMAF, CRF in the UI.

    Args:
        gui: The main GUI instance containing statistic display widgets
        info: Dictionary containing conversion progress information
    """
    if not info or not gui.conversion_running:
        logging.debug("Skipping update_conversion_statistics: no info or not running")
        return

    # Log received info for debugging
    logging.debug(f"Processing statistics update: {info}")

    # VMAF Updates
    if "vmaf" in info and info["vmaf"] is not None:
        try: # Ensure vmaf value is valid
            vmaf_val = float(info["vmaf"])
            vmaf_status = f"{vmaf_val:.1f}"
            if info.get("phase") == "crf-search": vmaf_status += " (Current)"
            if info.get("used_fallback"): vmaf_status += " (Fallback Used)" # Check if fallback was used
            update_ui_safely(gui.root, lambda v=vmaf_status: gui.vmaf_label.config(text=v))
            logging.info(f"VMAF update: {vmaf_status}")
        except (ValueError, TypeError) as e:
            logging.warning(f"Invalid VMAF value in info for update: {info.get('vmaf')} - {e}")
    elif info.get("phase") == "crf-search":
        # Reset to target if in search phase without a specific VMAF value yet
        vmaf_target = info.get("vmaf_target_used", DEFAULT_VMAF_TARGET)
        update_ui_safely(gui.root, lambda v=vmaf_target: gui.vmaf_label.config(text=f"{v} (Target)"))

    # CRF Updates
    if "crf" in info and info["crf"] is not None:
        try: # Ensure crf value is valid
            crf_val = int(info['crf'])
            # Use constant for preset
            settings_text = f"CRF: {crf_val}, Preset: {DEFAULT_ENCODING_PRESET}"
            update_ui_safely(gui.root, lambda s=settings_text: gui.encoding_settings_label.config(text=s))
            logging.info(f"Encoding settings update: {settings_text}")
        except (ValueError, TypeError) as e:
            logging.warning(f"Invalid CRF value in info for update: {info.get('crf')} - {e}")

    # ETA Calculation - Store AB-AV1's reported ETA and count down
    encoding_prog = info.get("progress_encoding", 0)
    gui.last_encoding_progress = encoding_prog  # Store for continuous ETA updates
    logging.debug(f"Encoding progress: {encoding_prog}, Phase: {info.get('phase', 'unknown')}")
    
    # Check if we have AB-AV1's ETA text
    if "eta_text" in info and info["eta_text"]:
        # Parse AB-AV1's ETA and store it with timestamp
        eta_seconds = parse_eta_text(info["eta_text"])
        # Only store non-zero ETAs or if we don't have one already
        if eta_seconds > 0 or not hasattr(gui, 'last_eta_seconds'):
            gui.last_eta_seconds = eta_seconds
            gui.last_eta_timestamp = time.time()
            logging.debug(f"Captured AB-AV1 ETA: {info['eta_text']} -> {eta_seconds} seconds")
    
    if encoding_prog > 0:
        if hasattr(gui, 'current_file_start_time') and gui.current_file_start_time:
            if not hasattr(gui, 'current_file_encoding_start_time') or not gui.current_file_encoding_start_time:
                # Check if encoding phase *just* started - check phase directly
                if info.get("phase") == "encoding": # Remove threshold check
                    gui.current_file_encoding_start_time = time.time()
                    logging.info("Encoding phase started - initializing timer")

        # Use the stored AB-AV1 ETA and count down
        if hasattr(gui, 'last_eta_seconds') and hasattr(gui, 'last_eta_timestamp'):
            elapsed_since_update = time.time() - gui.last_eta_timestamp
            remaining_eta = max(0, gui.last_eta_seconds - elapsed_since_update)
            eta_str = format_time(remaining_eta)
            
            # Update the display
            update_ui_safely(gui.root, lambda eta=eta_str: gui.eta_label.config(text=eta))
            logging.debug(f"ETA countdown: {eta_str} (base: {gui.last_eta_seconds}s, elapsed: {elapsed_since_update:.1f}s)")
            
        elif hasattr(gui, 'current_file_encoding_start_time') and gui.current_file_encoding_start_time:
            # Fallback: calculate based on progress if no AB-AV1 ETA is stored
            elapsed_encoding_time = time.time() - gui.current_file_encoding_start_time
            if encoding_prog > 0 and elapsed_encoding_time > 1:
                try:
                    total_encoding_time_est = (elapsed_encoding_time / encoding_prog) * 100
                    eta_seconds = total_encoding_time_est - elapsed_encoding_time
                    eta_str = format_time(eta_seconds)
                    update_ui_safely(gui.root, lambda eta=eta_str: gui.eta_label.config(text=eta))
                    logging.debug(f"ETA calculation fallback: {eta_str} (progress: {encoding_prog:.1f}%, elapsed: {format_time(elapsed_encoding_time)})")
                except ZeroDivisionError:
                    update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Calculating..."))
                except Exception as e:
                    logging.error(f"Error calculating ETA: {e}")
                    update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Error"))
            else:
                update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Calculating..."))
        else:
            update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Calculating..."))
    elif info.get("phase") == "crf-search":
        update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Detecting..."))
    else:
        update_ui_safely(gui.root, lambda: gui.eta_label.config(text="-"))

    # Size Prediction - prioritize actual file size estimates
    # Check if we have an estimated_output_size from the partial file
    if "output_size" in info and "original_size" in info and info.get("is_estimate", False):
        # This is from a real-time measurement of the partial output file
        current_size = info["output_size"]
        original_size = info["original_size"]
        if original_size is not None and original_size > 0 and current_size is not None:
            try: # Ensure values are valid numbers
                current_size_f = float(current_size)
                original_size_f = float(original_size)
                ratio = (current_size_f / original_size_f) * 100
                # Use the estimated size directly
                size_str = f"{format_file_size(current_size_f)} ({ratio:.1f}%) [Est]"
                # CRITICAL: Use explicit lambda to capture the current value
                update_ui_safely(gui.root, lambda s=size_str: gui.output_size_label.config(text=s))
                logging.info(f"Output size estimate (from partial file): {size_str}")
            except (ValueError, TypeError, ZeroDivisionError) as e:
                 logging.warning(f"Invalid size data for estimate: {current_size}, {original_size} - {e}")

    # Otherwise check if we have a size_reduction percentage (likely from ab-av1 final output)
    elif "size_reduction" in info and info["size_reduction"] is not None:
        # If we have size_reduction percentage but not actual sizes yet
        try:
            # Get original size from the file if available
            if hasattr(gui, 'last_input_size') and gui.last_input_size:
                original_size = gui.last_input_size
                size_reduction_f = float(info["size_reduction"]) # Ensure float
                size_percentage = 100.0 - size_reduction_f
                output_size_estimate = original_size * (size_percentage / 100.0)
                ratio = size_percentage
                size_str = f"{format_file_size(output_size_estimate)} ({ratio:.1f}%)"

                # Update the last_output_size for future use (like final completion summary)
                gui.last_output_size = output_size_estimate

                # CRITICAL: Use explicit lambda to capture the current value
                update_ui_safely(gui.root, lambda s=size_str: gui.output_size_label.config(text=s))

                logging.info(f"Output size prediction: {size_str} (reduction: {info['size_reduction']:.1f}%)")
        except (ValueError, TypeError, ZeroDivisionError) as e:
            logging.error(f"Error calculating output size from reduction: {e}")
        except Exception as e: # Catch other potential errors
             logging.error(f"Unexpected error calculating output size from reduction: {e}")

    # Direct size information if available (typically on completion)
    elif "output_size" in info and "original_size" in info:
        current_size = info["output_size"]
        original_size = info["original_size"]
        if original_size is not None and original_size > 0 and current_size is not None:
            try: # Ensure values are valid numbers
                current_size_f = float(current_size)
                original_size_f = float(original_size)
                ratio = (current_size_f / original_size_f) * 100
                size_str = f"{format_file_size(current_size_f)} ({ratio:.1f}%)"

                # CRITICAL: Use explicit lambda to capture the current value
                update_ui_safely(gui.root, lambda s=size_str: gui.output_size_label.config(text=s))

                logging.info(f"Output size update: {size_str}")
            except (ValueError, TypeError, ZeroDivisionError) as e:
                 logging.warning(f"Invalid size data for final update: {current_size}, {original_size} - {e}")


def update_elapsed_time(gui, start_time: float) -> None:
    """Update the elapsed time label for current file only.

    Args:
        gui: The main GUI instance containing the elapsed time label
        start_time: Timestamp when processing of the current file started
    """
    if not gui.conversion_running or (gui.stop_event and gui.stop_event.is_set()):
        gui.elapsed_timer_id = None
        return

    # During encoding phase, show time since encoding started
    if hasattr(gui, 'current_file_encoding_start_time') and gui.current_file_encoding_start_time:
        current_file_elapsed = time.time() - gui.current_file_encoding_start_time
    else:
        # During quality detection phase, show the full time
        current_file_elapsed = time.time() - start_time
    
    update_ui_safely(gui.root, lambda t=current_file_elapsed: gui.elapsed_label.config(text=format_time(t)))

    # Also update total elapsed time
    update_total_elapsed_time(gui)
    
    # Update ETA display if we're in encoding phase
    update_eta_display(gui)
    
    # Remove direct call to update_total_remaining_time since it's now on its own timer

    # Schedule next update
    gui.elapsed_timer_id = gui.root.after(1000, lambda: update_elapsed_time(gui, start_time))


def update_total_elapsed_time(gui) -> None:
    """Update the total elapsed time label for the entire conversion batch.

    Args:
        gui: The main GUI instance containing the total elapsed time label
    """
    if hasattr(gui, 'total_conversion_start_time') and gui.conversion_running:
        total_elapsed = time.time() - gui.total_conversion_start_time
        update_ui_safely(gui.root, lambda t=total_elapsed: gui.total_elapsed_label.config(text=format_time(t)))
    else:
        update_ui_safely(gui.root, lambda: gui.total_elapsed_label.config(text="-"))


def update_total_remaining_time(gui) -> None:
    """Update the total estimated remaining time label.
    
    Args:
        gui: The main GUI instance containing the total remaining time label
    """
    if not gui.conversion_running:
        update_ui_safely(gui.root, lambda: gui.total_remaining_label.config(text="-"))
        return
    
    try:
        # Get current file info if processing
        current_file_info = None
        if hasattr(gui, 'current_file_path') and gui.current_file_path:
            current_file_info = {'path': gui.current_file_path}
        
        # Estimate remaining time
        total_remaining = utils.estimate_remaining_time(gui, current_file_info)
        remaining_str = format_time(total_remaining) if total_remaining > 0 else "Calculating..."
        
        logging.debug(f"Total remaining time: {total_remaining}s, display: {remaining_str}")
        update_ui_safely(gui.root, lambda r=remaining_str: gui.total_remaining_label.config(text=r))
    except Exception as e:
        logging.error(f"Error updating total remaining time: {e}")
        update_ui_safely(gui.root, lambda: gui.total_remaining_label.config(text="Error"))


def update_statistics_summary(gui) -> None:
    """Update the overall statistics summary labels with historical data from conversion_history.json.

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
    
    # Load and process historical data
    try:
        history = load_history()
        
        if history:
            # Extract statistics from all historical conversions
            historical_vmaf = []
            historical_crf = []
            historical_reductions = []
            total_input_size = 0
            total_output_size = 0
            total_conversions = 0
            
            for record in history:
                # VMAF
                if 'final_vmaf' in record and record['final_vmaf'] is not None:
                    try:
                        historical_vmaf.append(float(record['final_vmaf']))
                    except (ValueError, TypeError):
                        pass
                        
                # CRF
                if 'final_crf' in record and record['final_crf'] is not None:
                    try:
                        historical_crf.append(int(record['final_crf']))
                    except (ValueError, TypeError):
                        pass
                        
                # Size reduction
                if 'reduction_percent' in record and record['reduction_percent'] is not None:
                    try:
                        historical_reductions.append(float(record['reduction_percent']))
                    except (ValueError, TypeError):
                        pass
                        
                # Accumulate total sizes
                if 'input_size_mb' in record and record['input_size_mb'] is not None:
                    try:
                        total_input_size += float(record['input_size_mb']) * (1024**2)
                    except (ValueError, TypeError):
                        pass
                        
                if 'output_size_mb' in record and record['output_size_mb'] is not None:
                    try:
                        total_output_size += float(record['output_size_mb']) * (1024**2)
                    except (ValueError, TypeError):
                        pass
                        
                total_conversions += 1
            
            # Calculate statistics from historical data
            if historical_vmaf:
                try:
                    avg_vmaf = statistics.mean(historical_vmaf)
                    min_vmaf = min(historical_vmaf)
                    max_vmaf = max(historical_vmaf)
                    vmaf_avg_text = f"Avg: {avg_vmaf:.1f}"
                    vmaf_range_text = f"(Range: {min_vmaf:.1f}-{max_vmaf:.1f})"
                    logging.debug(f"Historical VMAF stats: avg={avg_vmaf:.1f}, min={min_vmaf:.1f}, max={max_vmaf:.1f}")
                except statistics.StatisticsError:
                    if len(historical_vmaf) == 1:
                        vmaf_avg_text = f"{historical_vmaf[0]:.1f}"
                        vmaf_range_text = ""
                    else:
                        vmaf_avg_text = "Error"
                        vmaf_range_text = ""
                except Exception as e:
                    logging.warning(f"Error calculating historical VMAF stats: {e}")
                    vmaf_avg_text = "Error"
                    vmaf_range_text = ""
            
            if historical_crf:
                try:
                    avg_crf = statistics.mean(historical_crf)
                    min_crf = min(historical_crf)
                    max_crf = max(historical_crf)
                    crf_avg_text = f"Avg: {avg_crf:.1f}"
                    crf_range_text = f"(Range: {min_crf}-{max_crf})"
                    logging.debug(f"Historical CRF stats: avg={avg_crf:.1f}, min={min_crf}, max={max_crf}")
                except statistics.StatisticsError:
                    if len(historical_crf) == 1:
                        crf_avg_text = f"{historical_crf[0]}"
                        crf_range_text = ""
                    else:
                        crf_avg_text = "Error"
                        crf_range_text = ""
                except Exception as e:
                    logging.warning(f"Error calculating historical CRF stats: {e}")
                    crf_avg_text = "Error"
                    crf_range_text = ""
            
            if historical_reductions:
                try:
                    avg_reduction = statistics.mean(historical_reductions)
                    min_reduction = min(historical_reductions)
                    max_reduction = max(historical_reductions)
                    reduction_avg_text = f"Avg: {avg_reduction:.1f}%"
                    reduction_range_text = f"(Range: {min_reduction:.1f}%-{max_reduction:.1f}%)"
                    logging.info(f"Historical Size reduction stats: avg={avg_reduction:.1f}%, min={min_reduction:.1f}%, max={max_reduction:.1f}%")
                except statistics.StatisticsError:
                    if len(historical_reductions) == 1:
                        reduction_avg_text = f"{historical_reductions[0]:.1f}%"
                        reduction_range_text = ""
                    else:
                        reduction_avg_text = "Error"
                        reduction_range_text = ""
                except Exception as e:
                    logging.warning(f"Error calculating historical Size Reduction stats: {e}")
                    reduction_avg_text = "Error"
                    reduction_range_text = ""
            
            # Add total space saved and conversion count
            if total_input_size > 0 and total_output_size > 0:
                total_saved = total_input_size - total_output_size
                total_saved_text = f"{format_file_size(total_saved)} ({total_conversions} files)"
        
        else:
            # No history - show current session stats instead
            if gui.conversion_running or gui.processed_files > 0:
                if gui.vmaf_scores:
                    try:
                        avg_vmaf = statistics.mean(gui.vmaf_scores)
                        min_vmaf = min(gui.vmaf_scores)
                        max_vmaf = max(gui.vmaf_scores)
                        vmaf_avg_text = f"Avg: {avg_vmaf:.1f}"
                        vmaf_range_text = f"(Range: {min_vmaf:.1f}-{max_vmaf:.1f})"
                    except (statistics.StatisticsError, Exception):
                        if len(gui.vmaf_scores) == 1:
                            vmaf_avg_text = f"{gui.vmaf_scores[0]:.1f}"
                            vmaf_range_text = ""
                        else:
                            vmaf_avg_text = "Error"
                            vmaf_range_text = ""
                
                if gui.crf_values:
                    try:
                        avg_crf = statistics.mean(gui.crf_values)
                        min_crf = min(gui.crf_values)
                        max_crf = max(gui.crf_values)
                        crf_avg_text = f"Avg: {avg_crf:.1f}"
                        crf_range_text = f"(Range: {min_crf}-{max_crf})"
                    except (statistics.StatisticsError, Exception):
                        if len(gui.crf_values) == 1:
                            crf_avg_text = f"{gui.crf_values[0]}"
                            crf_range_text = ""
                        else:
                            crf_avg_text = "Error"
                            crf_range_text = ""
                
                if gui.size_reductions:
                    try:
                        valid_reductions = [r for r in gui.size_reductions if isinstance(r, (int, float))]
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
            else:
                vmaf_text = "No history"
                crf_text = "No history"
                reduction_text = "No history"
    
    except Exception as e:
        logging.error(f"Error loading historical statistics: {e}")
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
    logging.debug(f"Updating statistics summary - VMAF scores: {len(gui.vmaf_scores)}, CRF values: {len(gui.crf_values)}, Size reductions: {len(gui.size_reductions)}")

    if gui.vmaf_scores:
        try:
            avg_vmaf = statistics.mean(gui.vmaf_scores)
            min_vmaf = min(gui.vmaf_scores)
            max_vmaf = max(gui.vmaf_scores)
            vmaf_avg_text = f"Avg: {avg_vmaf:.1f}"
            vmaf_range_text = f"(Range: {min_vmaf:.1f}-{max_vmaf:.1f})"
            logging.debug(f"VMAF stats: avg={avg_vmaf:.1f}, min={min_vmaf:.1f}, max={max_vmaf:.1f}")
        except statistics.StatisticsError: # Handle case with insufficient data
            if len(gui.vmaf_scores) == 1: 
                vmaf_avg_text = f"{gui.vmaf_scores[0]:.1f}"
                vmaf_range_text = ""
            else: 
                vmaf_avg_text = "Error"
                vmaf_range_text = ""
                logging.warning("StatisticsError calculating VMAF stats")
        except Exception as e:
            logging.warning(f"Error calculating VMAF stats: {e}")
            vmaf_avg_text = "Error"
            vmaf_range_text = ""

    if gui.crf_values:
         try:
            avg_crf = statistics.mean(gui.crf_values)
            min_crf = min(gui.crf_values)
            max_crf = max(gui.crf_values)
            crf_avg_text = f"Avg: {avg_crf:.1f}"
            crf_range_text = f"(Range: {min_crf}-{max_crf})"
            logging.debug(f"CRF stats: avg={avg_crf:.1f}, min={min_crf}, max={max_crf}")
         except statistics.StatisticsError: # Handle case with insufficient data
             if len(gui.crf_values) == 1: 
                 crf_avg_text = f"{gui.crf_values[0]}"
                 crf_range_text = ""
             else: 
                 crf_avg_text = "Error"
                 crf_range_text = ""
                 logging.warning("StatisticsError calculating CRF stats")
         except Exception as e:
            logging.warning(f"Error calculating CRF stats: {e}")
            crf_avg_text = "Error"
            crf_range_text = ""

    if gui.size_reductions:
        try:
            # Make sure the list isn't empty and contains valid numbers
            valid_reductions = [r for r in gui.size_reductions if isinstance(r, (int, float))]
            if not valid_reductions:
                reduction_avg_text = "No data"
                reduction_range_text = ""
            else:
                avg_reduction = statistics.mean(valid_reductions)
                min_reduction = min(valid_reductions)
                max_reduction = max(valid_reductions)
                reduction_avg_text = f"Avg: {avg_reduction:.1f}%"
                reduction_range_text = f"(Range: {min_reduction:.1f}%-{max_reduction:.1f}%)"
                logging.info(f"Size reduction stats: avg={avg_reduction:.1f}%, min={min_reduction:.1f}%, max={max_reduction:.1f}%")
        except statistics.StatisticsError: # Handle case with insufficient data
             if len(valid_reductions) == 1: 
                 reduction_avg_text = f"{valid_reductions[0]:.1f}%"
                 reduction_range_text = ""
             else: 
                 reduction_avg_text = "Error"
                 reduction_range_text = ""
                 logging.warning("StatisticsError calculating Size Reduction stats")
        except Exception as e:
            logging.warning(f"Error calculating Size Reduction stats: {e}")
            logging.warning(f"Size reduction values: {gui.size_reductions}")
            reduction_avg_text = "Error"
            reduction_range_text = ""

    # Calculate total saved for current session
    if hasattr(gui, 'total_input_bytes_success') and hasattr(gui, 'total_output_bytes_success'):
        if gui.total_input_bytes_success > 0 and gui.total_output_bytes_success > 0:
            total_saved = gui.total_input_bytes_success - gui.total_output_bytes_success
            if total_saved > 0:
                total_saved_text = f"{format_file_size(total_saved)} ({gui.successful_conversions} files)"

    # Use lambdas with explicit parameter capturing for UI updates
    update_ui_safely(gui.root, lambda v=vmaf_avg_text: gui.vmaf_stats_label.config(text=v))
    update_ui_safely(gui.root, lambda v=vmaf_range_text: gui.vmaf_range_label.config(text=v))
    update_ui_safely(gui.root, lambda c=crf_avg_text: gui.crf_stats_label.config(text=c))
    update_ui_safely(gui.root, lambda c=crf_range_text: gui.crf_range_label.config(text=c))
    update_ui_safely(gui.root, lambda r=reduction_avg_text: gui.size_stats_label.config(text=r))
    update_ui_safely(gui.root, lambda r=reduction_range_text: gui.size_range_label.config(text=r))
    update_ui_safely(gui.root, lambda t=total_saved_text: gui.total_saved_label.config(text=t))


def update_eta_display(gui) -> None:
    """Update the ETA display based on stored AB-AV1 ETA or progress.
    This function is called every second to provide continuous countdown.
    """
    if not gui.conversion_running:
        return
    
    # Use the stored AB-AV1 ETA and count down
    if hasattr(gui, 'last_eta_seconds') and hasattr(gui, 'last_eta_timestamp'):
        elapsed_since_update = time.time() - gui.last_eta_timestamp
        remaining_eta = max(0, gui.last_eta_seconds - elapsed_since_update)
        eta_str = format_time(remaining_eta)
        update_ui_safely(gui.root, lambda eta=eta_str: gui.eta_label.config(text=eta))
        return
    
    # Fallback to calculation based on progress
    if not hasattr(gui, 'last_encoding_progress'):
        return
        
    encoding_prog = getattr(gui, 'last_encoding_progress', 0)
    
    if encoding_prog > 0 and hasattr(gui, 'current_file_encoding_start_time') and gui.current_file_encoding_start_time:
        elapsed_encoding_time = time.time() - gui.current_file_encoding_start_time
        if encoding_prog > 0 and elapsed_encoding_time > 1:
            try:
                total_encoding_time_est = (elapsed_encoding_time / encoding_prog) * 100
                eta_seconds = total_encoding_time_est - elapsed_encoding_time
                eta_str = format_time(eta_seconds)
                update_ui_safely(gui.root, lambda eta=eta_str: gui.eta_label.config(text=eta))
            except ZeroDivisionError:
                pass
            except Exception as e:
                logging.error(f"Error updating ETA display: {e}")


def reset_current_file_details(gui) -> None:
    """Reset labels related to the currently processing file to default values.

    Args:
        gui: The main GUI instance containing file detail UI elements
    """
    # Use constant imported from src.config
    default_vmaf_text = f"{DEFAULT_VMAF_TARGET} (Target)"
    update_ui_safely(gui.root, lambda: gui.current_file_label.config(text="No file processing"))
    update_ui_safely(gui.root, update_progress_bars, gui, 0, 0) # Use helper
    update_ui_safely(gui.root, lambda: gui.orig_format_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.orig_size_label.config(text="-"))
    update_ui_safely(gui.root, lambda v=default_vmaf_text: gui.vmaf_label.config(text=v))
    update_ui_safely(gui.root, lambda: gui.elapsed_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.eta_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.output_size_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.encoding_settings_label.config(text="-"))
    gui.current_file_encoding_start_time = None
    gui.last_encoding_progress = 0  # Reset last progress for ETA calculation
    # Reset stored AB-AV1 ETA values
    if hasattr(gui, 'last_eta_seconds'):
        del gui.last_eta_seconds
    if hasattr(gui, 'last_eta_timestamp'):
        del gui.last_eta_timestamp