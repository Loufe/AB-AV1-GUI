# tests/test_runner.py
"""Tests for run_ab_av1 (src/ab_av1/runner.py).

A FakeProcess stands in for subprocess.Popen: readline yields scripted lines
then EOFs or blocks, wait() honours an "exited" flag, and terminate_process_tree
is monkeypatched to record calls and (optionally) kill the fake. Timeouts are
shrunk so every path resolves in milliseconds.
"""

import subprocess
import threading

import pytest
import src.ab_av1.runner as runner_mod
from src.ab_av1.runner import run_ab_av1

FAST = {"silence_timeout_sec": 5.0, "poll_interval_sec": 0.01, "eof_wait_sec": 1.0, "terminate_wait_sec": 1.0}


class FakeProcess:
    """Scripted stand-in for subprocess.Popen with a merged stdout pipe."""

    def __init__(self, lines, rc=0, exits_after_eof=True, block_after_lines=False):
        self.pid = 4242
        self.returncode = None
        self.stdout = self
        self.kill_called = False
        self.wait_calls = 0
        self._rc = rc
        self._lines = list(lines)
        self._block = block_after_lines
        self._unblock = threading.Event()
        self._exited = threading.Event()
        if exits_after_eof and not block_after_lines:
            self._exited.set()

    # --- file-object interface (used via process.stdout) ---

    def readline(self):
        if self._lines:
            return self._lines.pop(0)
        if self._block:
            self._unblock.wait()
        return ""

    def close(self):
        pass

    # --- process interface ---

    def poll(self):
        return self.returncode

    def wait(self, timeout=None):
        self.wait_calls += 1
        if not self._exited.is_set():
            raise subprocess.TimeoutExpired("ab-av1", timeout or 0)
        self.returncode = self._rc
        return self.returncode

    def kill(self):
        self.kill_called = True
        self.die(rc=-9)

    def die(self, rc):
        """Simulate the process dying: unblock readline, let wait() return."""
        if self.returncode is None:
            self._rc = rc
        self._exited.set()
        self._unblock.set()


@pytest.fixture
def install(monkeypatch):
    """Install a FakeProcess as the runner's Popen; record tree-kill calls."""

    def _install(fake, kill_on_terminate=True):
        state = {"fake": fake, "tree_kills": []}

        def fake_popen(cmd, **kwargs):
            state["cmd"] = cmd
            return fake

        def fake_tree_kill(pid):
            state["tree_kills"].append(pid)
            if kill_on_terminate:
                fake.die(rc=-15)
            return True

        monkeypatch.setattr(runner_mod.subprocess, "Popen", fake_popen)
        monkeypatch.setattr(runner_mod, "terminate_process_tree", fake_tree_kill)
        return state

    return _install


def test_clean_run_filters_noise_and_reports_rc(install):
    fake = FakeProcess(["line one\n", "   \n", "[TRACE sled::pagecache] internal noise\n", "line two\n"], rc=0)
    state = install(fake)
    seen = []
    pids = []

    result = run_ab_av1(["ab-av1"], cwd=".", env={}, on_line=seen.append, pid_callback=pids.append, **FAST)

    assert result.return_code == 0
    assert seen == ["line one", "line two"]
    assert result.output == "line one\nline two"
    assert result.error_line is None
    assert result.cancelled is False
    assert result.silence_timeout is False
    assert pids == [4242]
    assert state["tree_kills"] == []


def test_on_line_exception_does_not_abort_run(install):
    fake = FakeProcess(["good\n", "boom\n", "after\n"], rc=0)
    install(fake)
    seen = []

    def on_line(line):
        seen.append(line)
        if line == "boom":
            raise ValueError("callback bug")

    result = run_ab_av1(["ab-av1"], cwd=".", env={}, on_line=on_line, **FAST)

    assert result.return_code == 0
    assert seen == ["good", "boom", "after"]
    assert result.output == "good\nboom\nafter"


def test_cancel_mid_stream_terminates_and_reaps(install):
    fake = FakeProcess([f"line {i}\n" for i in range(1, 6)], exits_after_eof=False, block_after_lines=True)
    state = install(fake)
    cancel = threading.Event()
    seen = []

    def on_line(line):
        seen.append(line)
        if len(seen) == 3:
            cancel.set()

    result = run_ab_av1(["ab-av1"], cwd=".", env={}, on_line=on_line, cancel_event=cancel, **FAST)

    assert result.cancelled is True
    assert result.silence_timeout is False
    assert seen == ["line 1", "line 2", "line 3"]
    assert state["tree_kills"] == [4242]
    assert fake.wait_calls >= 1
    assert fake.returncode is not None  # reaped


def test_cancel_during_silence_wakes_on_poll_interval(install):
    fake = FakeProcess([], exits_after_eof=False, block_after_lines=True)
    state = install(fake)
    cancel = threading.Event()
    cancel.set()

    result = run_ab_av1(["ab-av1"], cwd=".", env={}, cancel_event=cancel, **FAST)

    assert result.cancelled is True
    assert state["tree_kills"] == [4242]
    assert fake.returncode is not None


def test_silent_hung_process_hits_silence_timeout(install):
    fake = FakeProcess([], exits_after_eof=False, block_after_lines=True)
    state = install(fake)

    result = run_ab_av1(
        ["ab-av1"],
        cwd=".",
        env={},
        silence_timeout_sec=0.05,
        poll_interval_sec=0.01,
        eof_wait_sec=1.0,
        terminate_wait_sec=1.0,
    )

    assert result.silence_timeout is True
    assert result.cancelled is False
    assert state["tree_kills"] == [4242]
    assert fake.returncode is not None


def test_last_error_line_is_extracted(install):
    fake = FakeProcess(
        [
            "[2026-01-01T00:00:00Z DEBUG ab_av1] some log\n",
            "Error: something transient\n",
            "Error: Failed to find a suitable crf\n",
        ],
        rc=1,
    )
    install(fake)

    result = run_ab_av1(["ab-av1"], cwd=".", env={}, **FAST)

    assert result.return_code == 1
    assert result.error_line == "Failed to find a suitable crf"


def test_kill_escalation_when_tree_kill_does_not_work(install):
    fake = FakeProcess([], exits_after_eof=False, block_after_lines=True)
    state = install(fake, kill_on_terminate=False)
    cancel = threading.Event()
    cancel.set()

    result = run_ab_av1(
        ["ab-av1"],
        cwd=".",
        env={},
        cancel_event=cancel,
        silence_timeout_sec=5.0,
        poll_interval_sec=0.01,
        eof_wait_sec=1.0,
        terminate_wait_sec=0.01,
    )

    assert result.cancelled is True
    assert state["tree_kills"] == [4242]
    assert fake.kill_called is True
    assert fake.returncode is not None  # reaped after kill()


def test_out_of_band_kill_with_cancel_set_reports_cancelled(install):
    # Force-stop backstop race: an external PID kill closes the pipe (EOF) before
    # the poll loop ever sees cancel_event. The nonzero rc must still be reported
    # as a cancellation, not a failure.
    fake = FakeProcess([], rc=-15)
    state = install(fake)
    cancel = threading.Event()
    cancel.set()

    # Large poll interval: the loop can only wake via the reader's EOF sentinel,
    # guaranteeing we exercise the EOF path rather than the poll-tick cancel check.
    result = run_ab_av1(
        ["ab-av1"],
        cwd=".",
        env={},
        cancel_event=cancel,
        silence_timeout_sec=30.0,
        poll_interval_sec=30.0,
        eof_wait_sec=1.0,
        terminate_wait_sec=1.0,
    )

    assert result.cancelled is True
    assert result.return_code == -15
    assert state["tree_kills"] == []  # killed out-of-band, not by the runner


def test_clean_exit_with_cancel_set_is_not_cancelled(install):
    # A genuine rc=0 success races a late cancel: the completed work wins.
    fake = FakeProcess([], rc=0)
    install(fake)
    cancel = threading.Event()
    cancel.set()

    result = run_ab_av1(
        ["ab-av1"],
        cwd=".",
        env={},
        cancel_event=cancel,
        silence_timeout_sec=30.0,
        poll_interval_sec=30.0,
        eof_wait_sec=1.0,
        terminate_wait_sec=1.0,
    )

    assert result.cancelled is False
    assert result.return_code == 0


def test_error_line_not_extracted_on_success(install):
    fake = FakeProcess(["Error: transient warning that did not kill the run\n"], rc=0)
    install(fake)

    result = run_ab_av1(["ab-av1"], cwd=".", env={}, **FAST)

    assert result.return_code == 0
    assert result.error_line is None


def test_eof_without_exit_terminates_after_wait(install):
    fake = FakeProcess(["only line\n"], exits_after_eof=False)
    state = install(fake)

    result = run_ab_av1(
        ["ab-av1"],
        cwd=".",
        env={},
        silence_timeout_sec=5.0,
        poll_interval_sec=0.01,
        eof_wait_sec=0.01,
        terminate_wait_sec=1.0,
    )

    assert result.cancelled is False
    assert result.silence_timeout is False
    assert state["tree_kills"] == [4242]
    assert result.return_code == -15
    assert result.output == "only line"
