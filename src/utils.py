#src/utils.py
"""
Utility functions for the AV1 Video Converter application.
"""
import os
import datetime
import logging
from logging.handlers import RotatingFileHandler
import subprocess
import json
import shutil
import sys # Needed for sys.argv access
import time  # Add time import for estimate_remaining_time
import re  # Need for parse_eta_text

# Logging setup
logger = logging.getLogger(__name__)

# --- ETA Parsing Function ---

def parse_eta_text(eta_text: str) -> float:
    """Parse ETA text from AB-AV1 into seconds.
    
    Args:
        eta_text: ETA string like "2 hours", "87 minutes", "3h 20m", etc.
    Returns:
        Seconds remaining, or 0 if unparseable
    """
    if not eta_text:
        return 0
    
    try:
        eta_lower = eta_text.lower()
        
        # Handle simple formats first
        if 'hour' in eta_lower and 'min' not in eta_lower:
            # Format: "2 hours" or "1 hour"
            hours = float(re.search(r'(\d+(\.\d+)?)', eta_lower).group(1))
            return hours * 3600
        elif 'minute' in eta_lower and 'hour' not in eta_lower:
            # Format: "87 minutes" or "1 minute"
            minutes = float(re.search(r'(\d+(\.\d+)?)', eta_lower).group(1))
            return minutes * 60
        elif 'second' in eta_lower and 'hour' not in eta_lower and 'min' not in eta_lower:
            # Format: "30 seconds"
            seconds = float(re.search(r'(\d+(\.\d+)?)', eta_lower).group(1))
            return seconds
        elif 'h' in eta_lower and 'm' in eta_lower:
            # Format: "3h 20m"
            match = re.match(r'(\d+)h\s*(\d+)m', eta_lower)
            if match:
                hours = int(match.group(1))
                minutes = int(match.group(2))
                return hours * 3600 + minutes * 60
        
        # More complex formats
        # Extract all numbers and units
        parts = re.findall(r'(\d+(?:\.\d+)?)\s*(hour|minute|second|h|m|s)', eta_lower)
        total_seconds = 0
        
        for value, unit in parts:
            value = float(value)
            if unit.startswith('h'):
                total_seconds += value * 3600
            elif unit.startswith('m'):
                total_seconds += value * 60
            elif unit.startswith('s'):
                total_seconds += value
        
        return total_seconds if total_seconds > 0 else 0
    except Exception as e:
        logging.warning(f"Could not parse ETA text '{eta_text}': {e}")
        return 0

# --- Constants and Formatting Functions ---
# Moved to src/config.py: DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET

def format_time(seconds: float) -> str:
    """Format time in seconds to hours:minutes:seconds.
    
    Args:
        seconds: Time duration in seconds
        
    Returns:
        Formatted time string in the format of "h:mm:ss" or "m:ss"
    """
    if seconds is None or seconds < 0: return "--:--:--"
    hours = int(seconds // 3600)
    minutes = int((seconds % 3600) // 60)
    secs = int(seconds % 60)
    if hours > 0: return f"{hours}:{minutes:02d}:{secs:02d}"
    else: return f"{minutes}:{secs:02d}"

def format_file_size(size_bytes: int) -> str:
    """Format file size from bytes to appropriate unit (KB, MB, GB).
    
    Args:
        size_bytes: File size in bytes
        
    Returns:
        Formatted size string with appropriate unit (B, KB, MB, GB)
    """
    if size_bytes is None or size_bytes < 0: return "-"
    if size_bytes < 1024: return f"{size_bytes} B"
    elif size_bytes < 1024**2: return f"{size_bytes/1024:.2f} KB"
    elif size_bytes < 1024**3: return f"{size_bytes/(1024**2):.2f} MB"
    else: return f"{size_bytes/(1024**3):.2f} GB"

# --- Filename Anonymization ---
# File mapping for filename anonymization
filename_size_map = {}

def anonymize_filename(filename: str) -> str:
    """Replace actual filename with a simplified name for privacy reasons.
    
    Args:
        filename: Original file path or name to anonymize
        
    Returns:
        Anonymized filename, either basename or size-based representation
    """
    if not filename: return filename # Handle None input gracefully
    if filename in filename_size_map: return filename_size_map[filename]

    anonymized_name = os.path.basename(filename) # Default to basename
    try:
        if os.path.exists(filename):
            # File exists: Use size-based anonymization
            file_size_bytes = os.path.getsize(filename); file_size_mb = file_size_bytes / (1024**2)
            file_ext = os.path.splitext(filename)[1].lower(); anonymized_name = f"video_{file_size_mb:.1f}MB{file_ext}"
        else:
            # File doesn't exist: Just use the basename, no "file_" prefix
            anonymized_name = os.path.basename(filename)

        # Store mapping using original full path as key, even if returning basename
        filename_size_map[filename] = anonymized_name
        return anonymized_name
    except Exception as e:
        logging.debug(f"Error anonymizing '{filename}': {e}")
        return os.path.basename(filename) # Fallback

# Custom filter to replace filenames in log messages
class FilenamePrivacyFilter(logging.Filter):
    def filter(self, record):
        if hasattr(record, 'msg') and isinstance(record.msg, str):
            temp_msg = record.msg; sorted_keys = sorted(filename_size_map.keys(), key=len, reverse=True)
            for original_name in sorted_keys:
                # Get the *potentially* anonymized name from map (could be size-based or just basename)
                anonymized_name = filename_size_map[original_name]
                if original_name in temp_msg:
                    # Replace the original full path/name with the corresponding map value
                    temp_msg = temp_msg.replace(original_name, anonymized_name)
                else:
                    # If full path wasn't found, try replacing just the basename
                    original_basename = os.path.basename(original_name)
                    if original_basename != original_name and original_basename in temp_msg:
                         # Use the basename of the map value for replacement
                         anonymized_basename = os.path.basename(anonymized_name)
                         temp_msg = temp_msg.replace(original_basename, anonymized_basename)
            record.msg = temp_msg
        return True


# Custom filter to suppress excessive sled::pagecache trace messages
class SledTraceFilter(logging.Filter):
    def filter(self, record):
        if hasattr(record, 'msg') and isinstance(record.msg, str):
            # Filter out sled::pagecache messages marked as TRACE
            if 'sled::pagecache' in record.msg and 'TRACE' in record.msg:
                return False
        return True

# --- Logging Setup and Utilities ---

def get_script_directory() -> str:
    """Get the directory containing the main script/executable.
    
    Returns:
        Absolute path to the directory containing the main script or executable
    """
    if getattr(sys, 'frozen', False): return os.path.dirname(sys.executable)
    elif '__file__' in globals(): 
        script_path = os.path.abspath(__file__)
        # Navigate up one level from src/utils.py to the script directory
        return os.path.dirname(os.path.dirname(script_path))
    elif sys.argv and sys.argv[0]: 
        # Fallback using argv[0], might be less reliable depending on how it's run
        return os.path.dirname(os.path.abspath(sys.argv[0]))
    else: 
        # Last resort fallback
        return os.getcwd()


def setup_logging(log_directory: str = None, anonymize: bool = True) -> str:
    """Set up logging to file and console. Defaults log dir next to script.
    
    Args:
        log_directory: Optional path to log directory. If None, uses 'logs' folder next to script
        anonymize: Whether to anonymize filenames in logs for privacy
        
    Returns:
        The actual log directory path used, or None if setup failed
    """
    # (Unchanged from previous version)
    actual_log_directory_used = None
    try:
        if log_directory and os.path.isdir(log_directory): 
            logs_dir = os.path.abspath(log_directory)
            print(f"Using custom log directory: {logs_dir}")
        else:
            script_dir = get_script_directory()
            logs_dir = os.path.join(script_dir, "logs")
            logs_dir = os.path.abspath(logs_dir)
            if log_directory: 
                print(f"Warning: Custom log dir '{log_directory}' invalid. Using default: {logs_dir}")
            else: 
                print(f"Using default log directory: {logs_dir}")
        actual_log_directory_used = logs_dir
        os.makedirs(logs_dir, exist_ok=True)
    except Exception as e: print(f"ERROR: Cannot create/access log directory '{logs_dir}': {e}", file=sys.stderr); actual_log_directory_used = None
    log_file = None
    if actual_log_directory_used: 
        timestamp = datetime.datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
        log_file = os.path.join(actual_log_directory_used, f"av1_convert_{timestamp}.log")
    log_formatter = logging.Formatter('%(asctime)s - %(levelname)s - %(message)s'); file_handler = None
    if log_file:
        try:
            file_handler = RotatingFileHandler(log_file, maxBytes=10*1024*1024, backupCount=5, encoding='utf-8')
            file_handler.setFormatter(log_formatter); file_handler.setLevel(logging.DEBUG)
            if anonymize: file_handler.addFilter(FilenamePrivacyFilter())
            print(f"Log anonymization: {'Enabled' if anonymize else 'Disabled'}")
        except Exception as e: print(f"ERROR: Cannot create log file handler: {e}", file=sys.stderr); file_handler = None
    console_handler = logging.StreamHandler(); console_handler.setFormatter(log_formatter); console_handler.setLevel(logging.INFO)
    if anonymize: console_handler.addFilter(FilenamePrivacyFilter())
    logger = logging.getLogger(); logger.setLevel(logging.DEBUG)
    for handler in logger.handlers[:]:
        try: handler.close(); logger.removeHandler(handler)
        except: pass
    if file_handler: logger.addHandler(file_handler)
    logger.addHandler(console_handler)
    
    # Add the SledTraceFilter to reduce noise from sled::pagecache messages
    sled_filter = SledTraceFilter()
    logger.addFilter(sled_filter)
    
    logging.info(f"Filename anonymization in logs is {'ENABLED' if anonymize else 'DISABLED'}.")
    return actual_log_directory_used

# --- Video and FFmpeg Utilities ---
def log_video_properties(video_info: dict, prefix: str = "Input") -> None:
    """Log video file properties including format, codecs, resolution, etc.
    
    Args:
        video_info: Dictionary containing video metadata from ffprobe
        prefix: Prefix to use in log messages (e.g., "Input" or "Output")
    """
    if not video_info: 
        logging.warning(f"{prefix} video info not available")
        return
    file_size = video_info.get('file_size', 0); format_info = video_info.get('format', {})
    duration_str = format_info.get('duration', '0'); bit_rate_str = format_info.get('bit_rate', '0')
    try:
        duration = float(duration_str)
        bit_rate = int(bit_rate_str)
    except (ValueError, TypeError):
        duration = 0
        bit_rate = 0
    logging.info(f"{prefix} File - Size: {format_file_size(file_size)}, Duration: {format_time(duration)}, Bitrate: {bit_rate/1000:.2f} kbps")
    for stream in video_info.get('streams', []):
        codec_type = stream.get('codec_type', 'unknown'); codec_name = stream.get('codec_name', 'unknown')
        if codec_type == 'video':
            width = stream.get('width', 0); height = stream.get('height', 0); fps = stream.get('r_frame_rate', '0/1')
            try:
                if '/' in fps: num, den = map(int, fps.split('/')); fps_value = num / den if den != 0 else 0
                else: fps_value = float(fps)
            except: fps_value = 0
            profile = stream.get('profile', 'unknown'); pix_fmt = stream.get('pix_fmt', 'unknown')
            logging.info(f"{prefix} Video - {width}x{height} ({width*height/1000000:.2f} MP), {fps_value:.3f} fps, Codec: {codec_name}, Profile: {profile}, Pixel Format: {pix_fmt}")
        elif codec_type == 'audio':
            channels = stream.get('channels', 0); sample_rate = stream.get('sample_rate', 0); audio_bitrate_str = stream.get('bit_rate', '0')
            try:
                audio_bitrate = int(audio_bitrate_str) / 1000 # kbps
            except (ValueError, TypeError): 
                audio_bitrate = 0
            logging.info(f"{prefix} Audio - Codec: {codec_name}, Channels: {channels}, Sample Rate: {sample_rate} Hz, Bitrate: {audio_bitrate:.1f} kbps")

def log_encoding_parameters(crf: int, preset: str, width: int, height: int, vmaf_target: float) -> None:
    """Log encoding parameters used for the video conversion.
    
    Args:
        crf: Constant Rate Factor value
        preset: Encoding preset name/value
        width: Video width in pixels
        height: Video height in pixels
        vmaf_target: Target VMAF score for quality
    """
    resolution_name = "4K" if width >= 3840 or height >= 2160 else "1080p" if width >= 1920 or height >= 1080 else "720p" if width >= 1280 or height >= 720 else "SD"
    logging.info(f"Encoding Parameters - Res: {resolution_name} ({width}x{height}), CRF: {crf}, Preset: {preset}, VMAF Target: {vmaf_target}") # Log actual target used

def get_video_info(video_path: str) -> dict:
    """Get video file information using ffprobe.
    
    Args:
        video_path: Path to the video file to analyze
        
    Returns:
        Dictionary containing video metadata or None if analysis failed
    """
    cmd = ["ffprobe", "-v", "quiet", "-print_format", "json", "-show_format", "-show_streams", video_path]
    try:
        startupinfo = None;
        if os.name == 'nt': startupinfo = subprocess.STARTUPINFO(); startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW; startupinfo.wShowWindow = subprocess.SW_HIDE
        result = subprocess.run(cmd, capture_output=True, text=True, check=True, startupinfo=startupinfo, encoding='utf-8')
        info = json.loads(result.stdout)
        try:
            info['file_size'] = os.path.getsize(video_path)
        except Exception as e: 
            info['file_size'] = 0
            logging.debug(f"No size for {video_path}: {e}")
        return info
    except subprocess.CalledProcessError as e: logging.error(f"ffprobe failed for {anonymize_filename(video_path)}: {e.stderr}"); return None
    except json.JSONDecodeError as e: logging.error(f"ffprobe JSON error for {anonymize_filename(video_path)}: {e}"); return None
    except FileNotFoundError: logging.error(f"ffprobe not found."); return None
    except Exception as e: logging.error(f"ffprobe unexpected error {anonymize_filename(video_path)}: {e}"); return None

def check_ffmpeg_availability() -> tuple:
    """Check if FFmpeg is installed and has SVT-AV1 support.
    
    Returns:
        Tuple of (ffmpeg_available, svt_av1_available, version_info, error_message)
    """
    if shutil.which("ffmpeg") is None: return False, False, None, "ffmpeg not found in PATH"
    try:
        # First try to find and kill all child processes
        startupinfo = subprocess.STARTUPINFO()
        startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startupinfo.wShowWindow = subprocess.SW_HIDE
        
        result = subprocess.run(["ffmpeg", "-encoders"], 
                              capture_output=True, 
                              text=True, 
                              check=True, 
                              startupinfo=startupinfo, 
                              encoding='utf-8')
                              
        svt_av1_available = "libsvtav1" in result.stdout
        version_info = None
        try:
            version_result = subprocess.run(["ffmpeg", "-version"], 
                                         capture_output=True, 
                                         text=True, 
                                         startupinfo=startupinfo, 
                                         encoding='utf-8')
            if version_result.stdout: 
                version_info = version_result.stdout.splitlines()[0]
        except Exception as version_err: 
            # Just log and continue if version check fails
            logging.debug(f"Failed to get FFmpeg version: {version_err}")
            pass
        return True, svt_av1_available, version_info, None
    except Exception as e: return True, False, None, str(e)

# For UI updates
import tkinter as tk

def update_ui_safely(root: tk.Tk, update_function: callable, *args, **kwargs) -> None:
    """Thread-safe UI update with extra safety checks and logging.
    
    Args:
        root: Tkinter root window object
        update_function: Function to call on the UI thread
        *args: Arguments to pass to the update function
        **kwargs: Keyword arguments to pass to the update function
    """
    if root and root.winfo_exists():
        try:
            # Create a wrapper to capture what we're updating
            def _safe_update_wrapper():
                try:
                    result = update_function(*args, **kwargs)
                    logging.debug(f"UI update successful: {update_function.__name__ if hasattr(update_function, '__name__') else 'lambda'}")
                    return result
                except Exception as inner_e:
                    logging.error(f"Error in UI update function {update_function.__name__ if hasattr(update_function, '__name__') else 'lambda'}: {inner_e}")
            
            # Schedule on the main thread
            root.after(0, _safe_update_wrapper)
        except Exception as e:
            logging.error(f"Error scheduling UI update for {update_function.__name__ if hasattr(update_function, '__name__') else 'lambda'}: {e}")
    else:
        logging.debug(f"Skipped UI update: root widget invalid or destroyed")

def log_conversion_result(input_path: str, output_path: str, elapsed_time: float) -> None:
    """Log the results of a successful conversion including size reduction and time.
    
    Args:
        input_path: Path to the input video file
        output_path: Path to the output converted video
        elapsed_time: Time taken for conversion in seconds
    """
    if not os.path.exists(output_path): 
        logging.error(f"Result log failed - Output missing: {anonymize_filename(output_path)}")
        return
    try:
        input_size = os.path.getsize(input_path)
        output_size = os.path.getsize(output_path)
        
        # Calculate ratio and reduction percentage
        if input_size <= 0: 
            ratio = 0
            size_reduction_percent = 0
        else: 
            ratio = (output_size/input_size)*100
            size_reduction_percent = 100-ratio
            
        size_reduction = input_size-output_size
        input_info = get_video_info(input_path)
        output_info = get_video_info(output_path)
        input_bitrate = 0
        output_bitrate = 0
        resolution = ""
        if input_info and 'format' in input_info and 'bit_rate' in input_info['format']:
            try: 
                input_bitrate = int(input_info['format']['bit_rate'])/1000
            except (ValueError, TypeError):
                # Handle potential conversion errors
                logging.debug("Could not convert input bitrate to integer")
                pass
            for stream in input_info.get('streams',[]):
                if stream.get('codec_type')=='video': width=stream.get('width',0); height=stream.get('height',0); resolution=f"{width}x{height}"; break
        if output_info and 'format' in output_info and 'bit_rate' in output_info['format']:
            try: output_bitrate=int(output_info['format']['bit_rate'])/1000
            except: pass
        logging.info(f"Conversion Result [{anonymize_filename(output_path)}]: Input: {format_file_size(input_size)}, Output: {format_file_size(output_size)}, Reduction: {format_file_size(size_reduction)} ({size_reduction_percent:.2f}%), Time: {format_time(elapsed_time)}")
        if input_bitrate > 0 and output_bitrate > 0:
            bitrate_reduction=input_bitrate-output_bitrate; bitrate_ratio=(output_bitrate/input_bitrate)*100 if input_bitrate > 0 else 0
            logging.info(f"Bitrate Details - Input: {input_bitrate:.2f} kbps, Output: {output_bitrate:.2f} kbps, Reduction: {bitrate_reduction:.2f} kbps ({100-bitrate_ratio:.2f}%), Res: {resolution}")
        print(f"Conversion complete [{anonymize_filename(output_path)}] - Size reduced by {size_reduction_percent:.2f}% from {format_file_size(input_size)} to {format_file_size(output_size)} in {format_time(elapsed_time)}")
    except Exception as e: 
        logging.error(f"Error logging conversion result for {anonymize_filename(output_path)}: {e}")

# --- History Management Functions ---
HISTORY_FILE = "conversion_history.json"
def get_history_file_path() -> str:
    """Get the path to the conversion history JSON file.
    
    Returns:
        Absolute path to the history file
    """
    return os.path.join(get_script_directory(), HISTORY_FILE)
def load_history() -> list:
    """Load the conversion history from the JSON file.
    
    Returns:
        List of conversion history records or empty list if file doesn't exist
    """
    history_path = get_history_file_path()
    if os.path.exists(history_path):
        try:
            with open(history_path, 'r', encoding='utf-8') as f: 
                content = f.read()
                return json.loads(content) if content else []
        except (json.JSONDecodeError, OSError) as e: 
            logging.error(f"Error loading history {history_path}: {e}")
            return []
    return []
def append_to_history(record_dict: dict) -> None:
    """Append a new conversion record to the history file.
    
    Args:
        record_dict: Dictionary containing details about the conversion
    """
    history_path = get_history_file_path()
    try:
        history = load_history()
        history.append(record_dict)
        temp_history_path = history_path + ".tmp"
        
        with open(temp_history_path, 'w', encoding='utf-8') as f:
            json.dump(history, f, indent=2)
            
        os.replace(temp_history_path, history_path)
        logging.debug(f"Appended record to history: {history_path}")
    except OSError as e: 
        logging.error(f"Error saving history {history_path}: {e}")
    except Exception as e: 
        logging.error(f"Unexpected error appending history: {e}", exc_info=True)

# --- Power Management Functions ---
import ctypes

# Windows constants for SetThreadExecutionState
ES_CONTINUOUS = 0x80000000
ES_SYSTEM_REQUIRED = 0x00000001
ES_DISPLAY_REQUIRED = 0x00000002

def prevent_sleep_mode() -> bool:
    """Prevent the system from going to sleep while conversion is running.
    
    Returns:
        True if sleep prevention was successfully enabled, False otherwise
    """
    if sys.platform != "win32":
        logging.warning("Sleep prevention only supported on Windows")
        return False
        
    try:
        logging.info("Preventing system sleep during conversion")
        ctypes.windll.kernel32.SetThreadExecutionState(
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED
        )
        return True
    except Exception as e:
        logging.error(f"Failed to prevent system sleep: {e}")
        return False

def allow_sleep_mode() -> bool:
    """Restore normal power management behavior.
    
    Returns:
        True if sleep settings were successfully restored, False otherwise
    """
    if sys.platform != "win32":
        return False
        
    try:
        logging.info("Restoring normal power management")
        ctypes.windll.kernel32.SetThreadExecutionState(ES_CONTINUOUS)
        return True
    except Exception as e:
        logging.error(f"Failed to restore normal power management: {e}")
        return False


# --- Estimation Functions for Remaining Time ---

def find_similar_file_in_history(current_file_info: dict, tolerance: dict = None) -> dict:
    """Find a similar file in conversion history based on codec, duration, and size.
    
    Args:
        current_file_info: Dict with 'codec', 'duration', 'size' keys
        tolerance: Dict with tolerance values. Defaults to {'duration': 0.2, 'size': 0.3}
    Returns:
        Dict with historical processing data or None if no match found
    """
    if tolerance is None:
        tolerance = {'duration': 0.2, 'size': 0.3}  # 20% duration, 30% size tolerance
        
    history = load_history()
    if not history:
        return None
        
    best_match = None
    best_score = float('inf')
    
    current_codec = current_file_info.get('codec')
    current_duration = current_file_info.get('duration', 0)
    current_size = current_file_info.get('size', 0)
    
    for record in history:
        # Check if same codec
        hist_codec = record.get('input_codec') or record.get('input_vcodec')
        if hist_codec != current_codec:
            continue
            
        # Get metrics for comparison
        hist_duration = record.get('duration_sec', 0)
        hist_size = record.get('input_size_mb', 0) * (1024**2)  # Convert to bytes
        hist_time = record.get('time_sec', 0)
        
        if not (hist_duration and hist_size and hist_time):
            continue
            
        # Check if within tolerance
        if current_duration > 0 and hist_duration > 0:
            duration_diff = abs(hist_duration - current_duration) / hist_duration
        else:
            duration_diff = 1  # High value if can't compare
            
        if current_size > 0 and hist_size > 0:
            size_diff = abs(hist_size - current_size) / hist_size
        else:
            size_diff = 1  # High value if can't compare
        
        if duration_diff <= tolerance['duration'] and size_diff <= tolerance['size']:
            # Score based on similarity (lower is better)
            score = duration_diff + size_diff
            if score < best_score:
                best_score = score
                best_match = record
                
    return best_match


def estimate_processing_speed_from_history() -> float:
    """Calculate average processing speed (bytes/second) from historical data.
    
    Returns:
        Average processing speed in bytes/second or 0 if no history
    """
    history = load_history()
    if not history:
        return 0
        
    speeds = []
    for record in history:
        input_size = record.get('input_size_mb', 0) * (1024**2)  # Convert to bytes
        time_sec = record.get('time_sec', 0)
        if input_size > 0 and time_sec > 0:
            speeds.append(input_size / time_sec)
            
    return sum(speeds) / len(speeds) if speeds else 0


def estimate_remaining_time(gui, current_file_info: dict = None) -> float:
    """Estimate total remaining time for all queued files.
    
    Args:
        gui: The main GUI instance
        current_file_info: Dict with current file info if available
    Returns:
        Estimated remaining time in seconds
    """
    remaining_time = 0
    current_file_handled = False  # Track if we've already included the current file
    
    # If currently encoding, use the stored AB-AV1 ETA for the current file
    if getattr(gui, 'conversion_running', False):
        # Check for stored AB-AV1 ETA first
        if hasattr(gui, 'last_eta_seconds') and hasattr(gui, 'last_eta_timestamp'):
            elapsed_since_update = time.time() - gui.last_eta_timestamp
            current_eta = max(0, gui.last_eta_seconds - elapsed_since_update)
            remaining_time += current_eta
            current_file_handled = True  # Mark current file as handled
            logging.debug(f"Using AB-AV1 ETA: {current_eta}s for current file")
        # Fallback to calculation based on progress
        elif hasattr(gui, 'last_encoding_progress'):
            encoding_prog = getattr(gui, 'last_encoding_progress', 0)
            if encoding_prog > 0 and hasattr(gui, 'current_file_encoding_start_time') and gui.current_file_encoding_start_time:
                elapsed_encoding_time = time.time() - gui.current_file_encoding_start_time
                if encoding_prog > 0 and elapsed_encoding_time > 1:
                    total_encoding_time_est = (elapsed_encoding_time / encoding_prog) * 100
                    current_eta = total_encoding_time_est - elapsed_encoding_time
                    remaining_time += current_eta
                    current_file_handled = True  # Mark current file as handled
                    logging.debug(f"Using progress-based ETA: {current_eta}s for current file")
    
    # Get pending files including current file if in quality detection phase
    pending_files = getattr(gui, 'pending_files', [])
    current_path = getattr(gui, 'current_file_path', None)
    
    # Normalize current path for comparison
    if current_path:
        current_path = os.path.normpath(current_path)
    
    # If not in encoding phase, include current file in estimation
    if current_path and not getattr(gui, 'current_file_encoding_start_time', None):
        if current_path not in [os.path.normpath(p) for p in pending_files]:
            pending_files = [current_path] + pending_files
    
    # Debug logging to understand the issue
    logging.debug(f"Total remaining ETA calculation:")
    logging.debug(f"  Current file: {current_path}")
    logging.debug(f"  Current file handled: {current_file_handled}")
    logging.debug(f"  Pending files count: {len(pending_files)}")
    logging.debug(f"  Current remaining time before loop: {remaining_time}s")
    
    for file_path in pending_files:
        # Normalize path for comparison
        normalized_file_path = os.path.normpath(file_path)
        
        # Check if this is the current file and we've already handled it
        is_current_file = (normalized_file_path == current_path)
        
        if is_current_file and current_file_handled:
            logging.debug(f"Skipping current file {file_path} - already handled")
            continue
            
        # For current file in quality detection, try using historical data first
        if is_current_file:
            # If we've already handled the current file, absolutely skip it
            if current_file_handled:
                continue
            
            # If in quality detection phase, estimate this file
            if not getattr(gui, 'current_file_encoding_start_time', None):
                # Try to get file size from existing attributes
                file_size = getattr(gui, 'last_input_size', 0)
                file_codec = None
                file_duration = 0
            
                if file_size == 0:
                    # Get file info directly if available
                    try:
                        file_info = get_video_info(file_path)
                        if file_info:
                            file_size = file_info.get('file_size', 0)
                            # Extract codec and duration
                            for stream in file_info.get('streams', []):
                                if stream.get('codec_type') == 'video':
                                    file_codec = stream.get('codec_name')
                                    break
                            if 'format' in file_info and 'duration' in file_info['format']:
                                try:
                                    file_duration = float(file_info['format']['duration'])
                                except:
                                    file_duration = 0
                    except:
                        pass
                        
                    if file_size == 0:
                        try:
                            file_size = os.path.getsize(file_path)
                        except:
                            pass
                
                # Try historical data first for quality detection phase
                if file_size > 0 and file_codec and file_duration > 0:
                    similar_file = find_similar_file_in_history({
                        'codec': file_codec,
                        'duration': file_duration,
                        'size': file_size
                    })
                    
                    if similar_file:
                        # Use time from similar file
                        remaining_time += similar_file.get('time_sec', 0)
                        logging.debug(f"Found similar file for current quality detection {file_path}, estimated time: {similar_file.get('time_sec', 0)}s")
                        logging.debug(f"Total remaining time now: {remaining_time}s")
                        continue
                
                # Try average processing speed from history
                avg_speed = estimate_processing_speed_from_history()
                if avg_speed > 0 and file_size > 0:
                    time_est = file_size / avg_speed
                    remaining_time += time_est
                    logging.debug(f"Using average speed for current quality detection {file_path}, estimated time: {time_est}s, size: {file_size}, speed: {avg_speed}")
                    continue
                
                # Fallback to rough estimate
                if file_size > 0:
                    # Use rough estimate of 1 GB per hour for quality detection and encoding
                    rough_speed = (1024**3) / 3600  # 1 GB per hour
                    time_est = file_size / rough_speed
                    remaining_time += time_est
                    logging.debug(f"Using rough estimate for current file {file_path}, estimated time: {time_est}s, size: {file_size}")
                else:
                    # Default fallback - assume 30 minutes if no size available
                    remaining_time += 1800
                    logging.debug(f"No size for current file {file_path}, using default 30 minutes")
                continue
            
        # Get file info
        file_info = get_video_info(file_path)
        if not file_info:
            continue
            
        # Extract codec, duration, and size
        file_codec = None
        file_duration = 0
        file_size = file_info.get('file_size', 0)
        
        for stream in file_info.get('streams', []):
            if stream.get('codec_type') == 'video':
                file_codec = stream.get('codec_name')
                break
                
        if 'format' in file_info and 'duration' in file_info['format']:
            try:
                file_duration = float(file_info['format']['duration'])
            except:
                file_duration = 0
                logging.debug(f"Could not extract duration for {file_path}")
        
        # First, try to find a similar file in history
        similar_file = find_similar_file_in_history({
            'codec': file_codec,
            'duration': file_duration,
            'size': file_size
        })
        
        if similar_file:
            # Use time from similar file
            remaining_time += similar_file.get('time_sec', 0)
            logging.debug(f"Found similar file for {file_path}, estimated time: {similar_file.get('time_sec', 0)}s")
            logging.debug(f"Total remaining time now: {remaining_time}s")
        else:
            # Use average processing speed as fallback
            avg_speed = estimate_processing_speed_from_history()
            if avg_speed > 0 and file_size > 0:
                time_est = file_size / avg_speed
                remaining_time += time_est
                logging.debug(f"Using average speed for {file_path}, estimated time: {time_est}s, size: {file_size}, speed: {avg_speed}")
                logging.debug(f"Total remaining time now: {remaining_time}s")
            else:
                # If no historical data, use a rough estimate of 1 GB per hour
                if file_size > 0:
                    rough_speed = (1024**3) / 3600  # 1 GB per hour
                    time_est = file_size / rough_speed
                    remaining_time += time_est
                    logging.debug(f"No history - using rough estimate for {file_path}, estimated time: {time_est}s, size: {file_size}")
                    logging.debug(f"Total remaining time now: {remaining_time}s")
                else:
                    logging.debug(f"No estimation available for {file_path}, avg_speed: {avg_speed}, file_size: {file_size}")
    
    logging.debug(f"Total estimated remaining time: {remaining_time}s")
    
    return remaining_time