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

# --- Constants ---
DEFAULT_VMAF_TARGET = 95
DEFAULT_ENCODING_PRESET = "6" # Corresponds to "Balanced"

def format_time(seconds):
    """Format time in seconds to hours:minutes:seconds"""
    if seconds is None or seconds < 0: return "--:--:--"
    hours = int(seconds // 3600); minutes = int((seconds % 3600) // 60); secs = int(seconds % 60)
    if hours > 0: return f"{hours}:{minutes:02d}:{secs:02d}"
    else: return f"{minutes}:{secs:02d}"

def format_file_size(size_bytes):
    """Format file size from bytes to appropriate unit (KB, MB, GB)"""
    if size_bytes is None or size_bytes < 0: return "-"
    if size_bytes < 1024: return f"{size_bytes} B"
    elif size_bytes < 1024**2: return f"{size_bytes/1024:.2f} KB"
    elif size_bytes < 1024**3: return f"{size_bytes/(1024**2):.2f} MB"
    else: return f"{size_bytes/(1024**3):.2f} GB"

filename_size_map = {}

def anonymize_filename(filename):
    """Replace actual filename with a simplified name for privacy reasons"""
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

def get_script_directory():
    """Get the directory containing the main script/executable."""
    if getattr(sys, 'frozen', False): return os.path.dirname(sys.executable)
    elif '__file__' in globals(): script_path = os.path.abspath(__file__); return os.path.dirname(os.path.dirname(script_path))
    elif sys.argv and sys.argv[0]: return os.path.dirname(os.path.abspath(sys.argv[0]))
    else: return os.getcwd()


def setup_logging(log_directory=None, anonymize=True):
    """Set up logging to file and console. Defaults log dir next to script."""
    # (Unchanged from previous version)
    actual_log_directory_used = None
    try:
        if log_directory and os.path.isdir(log_directory): logs_dir = os.path.abspath(log_directory); print(f"Using custom log directory: {logs_dir}")
        else:
            script_dir = get_script_directory(); logs_dir = os.path.join(script_dir, "logs"); logs_dir = os.path.abspath(logs_dir)
            if log_directory: print(f"Warning: Custom log dir '{log_directory}' invalid. Using default: {logs_dir}")
            else: print(f"Using default log directory: {logs_dir}")
        actual_log_directory_used = logs_dir; os.makedirs(logs_dir, exist_ok=True)
    except Exception as e: print(f"ERROR: Cannot create/access log directory '{logs_dir}': {e}", file=sys.stderr); actual_log_directory_used = None
    log_file = None
    if actual_log_directory_used: timestamp = datetime.datetime.now().strftime("%Y-%m-%d_%H-%M-%S"); log_file = os.path.join(actual_log_directory_used, f"av1_convert_{timestamp}.log")
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
    logger.addHandler(console_handler); logging.info(f"Filename anonymization in logs is {'ENABLED' if anonymize else 'DISABLED'}.")
    return actual_log_directory_used

# --- Video/FFmpeg Utilities (log_video_properties, etc.) remain unchanged ---
def log_video_properties(video_info, prefix="Input"):
    if not video_info: logging.warning(f"{prefix} video info not available"); return
    file_size = video_info.get('file_size', 0); format_info = video_info.get('format', {})
    duration_str = format_info.get('duration', '0'); bit_rate_str = format_info.get('bit_rate', '0')
    try: duration = float(duration_str); bit_rate = int(bit_rate_str)
    except: duration = 0; bit_rate = 0
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
            try: audio_bitrate = int(audio_bitrate_str) / 1000 # kbps
            except: audio_bitrate = 0
            logging.info(f"{prefix} Audio - Codec: {codec_name}, Channels: {channels}, Sample Rate: {sample_rate} Hz, Bitrate: {audio_bitrate:.1f} kbps")

def log_encoding_parameters(crf, preset, width, height, vmaf_target): # Added vmaf_target
    resolution_name = "4K" if width >= 3840 or height >= 2160 else "1080p" if width >= 1920 or height >= 1080 else "720p" if width >= 1280 or height >= 720 else "SD"
    logging.info(f"Encoding Parameters - Res: {resolution_name} ({width}x{height}), CRF: {crf}, Preset: {preset}, VMAF Target: {vmaf_target}") # Log actual target used

def get_video_info(video_path):
    cmd = ["ffprobe", "-v", "quiet", "-print_format", "json", "-show_format", "-show_streams", video_path]
    try:
        startupinfo = None;
        if os.name == 'nt': startupinfo = subprocess.STARTUPINFO(); startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW; startupinfo.wShowWindow = subprocess.SW_HIDE
        result = subprocess.run(cmd, capture_output=True, text=True, check=True, startupinfo=startupinfo, encoding='utf-8')
        info = json.loads(result.stdout)
        try: info['file_size'] = os.path.getsize(video_path)
        except Exception as e: info['file_size'] = 0; logging.debug(f"No size for {video_path}: {e}")
        return info
    except subprocess.CalledProcessError as e: logging.error(f"ffprobe failed for {anonymize_filename(video_path)}: {e.stderr}"); return None
    except json.JSONDecodeError as e: logging.error(f"ffprobe JSON error for {anonymize_filename(video_path)}: {e}"); return None
    except FileNotFoundError: logging.error(f"ffprobe not found."); return None
    except Exception as e: logging.error(f"ffprobe unexpected error {anonymize_filename(video_path)}: {e}"); return None

def check_ffmpeg_availability():
    if shutil.which("ffmpeg") is None: return False, False, None, "ffmpeg not found in PATH"
    try:
        startupinfo = None;
        if os.name == 'nt': startupinfo = subprocess.STARTUPINFO(); startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW; startupinfo.wShowWindow = subprocess.SW_HIDE
        result = subprocess.run(["ffmpeg", "-encoders"], capture_output=True, text=True, check=True, startupinfo=startupinfo, encoding='utf-8')
        svt_av1_available = "libsvtav1" in result.stdout; version_info = None
        try:
            version_result = subprocess.run(["ffmpeg", "-version"], capture_output=True, text=True, startupinfo=startupinfo, encoding='utf-8')
            if version_result.stdout: version_info = version_result.stdout.splitlines()[0]
        except Exception: pass
        return True, svt_av1_available, version_info, None
    except Exception as e: return True, False, None, str(e)

def update_ui_safely(root, update_function, *args, **kwargs):
    """Thread-safe UI update with extra safety checks and logging"""
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

def log_conversion_result(input_path, output_path, elapsed_time):
    if not os.path.exists(output_path): logging.error(f"Result log failed - Output missing: {anonymize_filename(output_path)}"); return
    try:
        input_size=os.path.getsize(input_path); output_size=os.path.getsize(output_path)
        if input_size <= 0: ratio=0; size_reduction_percent=0
        else: ratio=(output_size/input_size)*100; size_reduction_percent=100-ratio
        size_reduction=input_size-output_size; input_info=get_video_info(input_path); output_info=get_video_info(output_path)
        input_bitrate=0; output_bitrate=0; resolution=""
        if input_info and 'format' in input_info and 'bit_rate' in input_info['format']:
            try: input_bitrate=int(input_info['format']['bit_rate'])/1000
            except: pass
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
    except Exception as e: logging.error(f"Error logging conversion result for {anonymize_filename(output_path)}: {e}")

# --- History Functions ---
HISTORY_FILE = "conversion_history.json"
def get_history_file_path(): return os.path.join(get_script_directory(), HISTORY_FILE)
def load_history():
    history_path = get_history_file_path()
    if os.path.exists(history_path):
        try:
            with open(history_path, 'r', encoding='utf-8') as f: content = f.read(); return json.loads(content) if content else []
        except (json.JSONDecodeError, OSError) as e: logging.error(f"Error loading history {history_path}: {e}"); return []
    return []
def append_to_history(record_dict):
    history_path = get_history_file_path()
    try:
        history = load_history(); history.append(record_dict); temp_history_path = history_path + ".tmp"
        with open(temp_history_path, 'w', encoding='utf-8') as f: json.dump(history, f, indent=2)
        os.replace(temp_history_path, history_path); logging.debug(f"Appended record to history: {history_path}")
    except OSError as e: logging.error(f"Error saving history {history_path}: {e}")
    except Exception as e: logging.error(f"Unexpected error appending history: {e}", exc_info=True)