# src/ab_av1/runner.py
"""
Subprocess lifecycle for ab-av1: spawn, stream output, cancel, and reap.

A dedicated reader thread feeds lines into a queue so the main loop can wake
on a poll interval even while the process is silent - this is what makes
cancellation and hang detection work between output lines. The runner is
parser-agnostic: callers observe output through the ``on_line`` callback.
"""

import logging
import queue
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from src.config import (
    AB_AV1_EOF_WAIT_SEC,
    AB_AV1_OUTPUT_POLL_SEC,
    AB_AV1_SILENCE_TIMEOUT_SEC,
    AB_AV1_TERMINATE_WAIT_SEC,
)
from src.platform_utils import get_windows_subprocess_startupinfo, terminate_process_tree

logger = logging.getLogger(__name__)

# Sentinel the reader thread enqueues when the process closes its output pipe
_EOF = object()

# ab-av1's main.rs prints "Error: {err}" to stderr as its final act before exit(1).
# stderr is merged into stdout, so the failure reason is the last such line.
_ERROR_PREFIX = "Error: "


@dataclass
class ProcessResult:
    """Outcome of one ab-av1 process run."""

    return_code: int
    output: str  # merged stdout+stderr, noise-filtered, newline-joined
    error_line: str | None  # on failure, text after the LAST line starting with "Error: "; else None
    cancelled: bool = False
    silence_timeout: bool = False


def _extract_error_line(lines: list[str]) -> str | None:
    """Return the message of the last "Error: ..." line, or None."""
    for line in reversed(lines):
        if line.startswith(_ERROR_PREFIX):
            return line[len(_ERROR_PREFIX) :]
    return None


def _terminate_and_reap(process: Any, terminate_wait_sec: float) -> int:
    """Kill the process tree, wait until the child is reaped (escalating if needed).

    Returns:
        The process's exit code, or -1 if it could not be reaped.
    """
    terminate_process_tree(process.pid)
    try:
        process.wait(timeout=terminate_wait_sec)
    except subprocess.TimeoutExpired:
        logger.warning(f"Process {process.pid} still alive after tree kill; escalating to kill()")
        process.kill()
        try:
            process.wait(timeout=terminate_wait_sec)
        except subprocess.TimeoutExpired:
            logger.exception(f"Process {process.pid} could not be reaped after kill()")
    rc = process.poll()
    return rc if rc is not None else -1


def run_ab_av1(
    cmd: list[str],
    *,
    cwd: str,
    env: dict[str, str],
    on_line: Callable[[str], None] | None = None,
    cancel_event: Any | None = None,
    pid_callback: Callable[[int], None] | None = None,
    silence_timeout_sec: float = AB_AV1_SILENCE_TIMEOUT_SEC,
    poll_interval_sec: float = AB_AV1_OUTPUT_POLL_SEC,
    eof_wait_sec: float = AB_AV1_EOF_WAIT_SEC,
    terminate_wait_sec: float = AB_AV1_TERMINATE_WAIT_SEC,
) -> ProcessResult:
    """Run an ab-av1 command to completion, cancellation, or hang timeout.

    Args:
        cmd: Full command line (executable + args).
        cwd: Working directory for the process (where temp folders appear).
        env: Environment for the process.
        on_line: Called for each non-noise output line. Exceptions are logged
            and swallowed - a callback bug must never kill an hours-long encode.
        cancel_event: Event checked between lines and on every poll interval;
            when set, the process tree is terminated and ``cancelled`` is True.
        pid_callback: Called with the process PID right after spawn.
        silence_timeout_sec: No output for this long terminates the process
            and sets ``silence_timeout`` (RUST_LOG debug output makes a healthy
            run chatty, so prolonged silence means a hung process).
        poll_interval_sec: Wake interval for cancel/silence checks.
        eof_wait_sec: How long to wait for exit after the pipe closes.
        terminate_wait_sec: Reap wait after terminate/kill before escalating.

    Returns:
        ProcessResult with the exit code, collected output, and flags.

    Raises:
        FileNotFoundError: If the executable cannot be spawned.
    """
    startupinfo, creationflags = get_windows_subprocess_startupinfo()
    process = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        universal_newlines=True,
        bufsize=1,
        cwd=cwd,
        startupinfo=startupinfo,
        creationflags=creationflags,
        encoding="utf-8",
        errors="replace",
        env=env,
        # POSIX: make ab-av1 a session leader so terminate_process_tree can
        # SIGKILL the whole group (a hung ffmpeg can't forward signals).
        start_new_session=(sys.platform != "win32"),
    )

    if pid_callback:
        try:
            pid_callback(process.pid)
        except Exception:
            logger.exception("pid_callback failed")

    logger.info(f"ab-av1 process {process.pid} started. Reading output...")

    line_queue: queue.Queue = queue.Queue()
    stdout_stream = process.stdout
    assert stdout_stream is not None  # Guaranteed by stdout=PIPE  # noqa: S101

    def _reader() -> None:
        try:
            for raw_line in iter(stdout_stream.readline, ""):
                line_queue.put(raw_line)
        except Exception:
            logger.exception(f"Reader thread failed for process {process.pid}")
        finally:
            line_queue.put(_EOF)

    reader = threading.Thread(target=_reader, name="ab-av1-reader", daemon=True)
    reader.start()

    lines: list[str] = []
    cancelled = False
    silence_timeout = False
    last_output_time = time.monotonic()

    while True:
        try:
            item = line_queue.get(timeout=poll_interval_sec)
        except queue.Empty:
            if cancel_event is not None and cancel_event.is_set():
                cancelled = True
                break
            if time.monotonic() - last_output_time > silence_timeout_sec:
                silence_timeout = True
                break
            continue

        if item is _EOF:
            break

        last_output_time = time.monotonic()
        line = item.strip()
        if not line or "sled::pagecache" in line:
            continue
        lines.append(line)
        if on_line is not None:
            try:
                on_line(line)
            except Exception:
                logger.exception(f"on_line callback failed for line: '{line[:80]}'")
        if cancel_event is not None and cancel_event.is_set():
            cancelled = True
            break

    if cancelled or silence_timeout:
        reason = "cancelled" if cancelled else f"silent for over {silence_timeout_sec}s"
        logger.warning(f"Terminating ab-av1 process {process.pid} ({reason})")
        return_code = _terminate_and_reap(process, terminate_wait_sec)
    else:
        try:
            return_code = process.wait(timeout=eof_wait_sec)
        except subprocess.TimeoutExpired:
            logger.warning(f"Process {process.pid} did not exit within {eof_wait_sec}s after closing its pipe")
            return_code = _terminate_and_reap(process, terminate_wait_sec)

    # An out-of-band kill (e.g. the GUI's force-stop backstop) can close the pipe
    # before the poll loop sees cancel_event; latch cancellation so a user stop is
    # never reported as a failure. A genuine rc=0 success still wins.
    if not cancelled and return_code != 0 and cancel_event is not None and cancel_event.is_set():
        logger.info(f"Process {process.pid} exited rc={return_code} with cancel_event set; treating as cancelled")
        cancelled = True

    # Only close the pipe once the reader is done with it: closing while the
    # reader blocks in readline() would deadlock on the buffered-IO lock.
    reader.join(timeout=terminate_wait_sec)
    if reader.is_alive():
        logger.warning(f"Reader thread for process {process.pid} still alive; leaving stdout pipe open")
    else:
        try:
            if process.stdout:
                process.stdout.close()
        except Exception as e:
            logger.warning(f"Error closing process stdout pipe: {e}")

    logger.info(f"ab-av1 process {process.pid} finished with return code {return_code}")
    return ProcessResult(
        return_code=return_code,
        output="\n".join(lines),
        error_line=_extract_error_line(lines) if return_code != 0 else None,
        cancelled=cancelled,
        silence_timeout=silence_timeout,
    )
