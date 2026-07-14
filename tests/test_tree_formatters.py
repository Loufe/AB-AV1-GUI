# tests/test_tree_formatters.py
"""Characterization tests for the pure formatting/parsing functions in
src/gui/tree_formatters.py (the module has no tkinter dependency; the
tree-sorting helpers that take a gui object are not covered here)."""

import math

from src.gui.tree_formatters import (
    format_compact_time,
    format_efficiency,
    parse_efficiency_to_value,
    parse_size_to_bytes,
    parse_time_to_seconds,
)

GIB = 1024**3

# ---------------------------------------------------------------------------
# format_compact_time
# ---------------------------------------------------------------------------


def test_format_compact_time_zero_and_negative_show_dash():
    assert format_compact_time(0) == "—"
    assert format_compact_time(-5) == "—"


def test_format_compact_time_hours_and_minutes():
    assert format_compact_time(8100) == "2h 15m"  # default confidence "none": no prefix
    assert format_compact_time(8100, confidence="high") == "2h 15m"
    assert format_compact_time(8100, confidence="medium") == "~2h 15m"
    assert format_compact_time(8100, confidence="low") == "~~2h 15m"


def test_format_compact_time_minutes_only():
    assert format_compact_time(2700) == "45m"
    assert format_compact_time(2700, confidence="medium") == "~45m"


def test_format_compact_time_under_one_minute():
    assert format_compact_time(30) == "< 1m"
    assert format_compact_time(30, confidence="medium") == "~< 1m"
    assert format_compact_time(30, confidence="low") == "~~< 1m"


# ---------------------------------------------------------------------------
# format_efficiency
# ---------------------------------------------------------------------------


def test_format_efficiency_invalid_inputs_show_dash():
    assert format_efficiency(0, 3600) == "—"
    assert format_efficiency(-100, 3600) == "—"
    assert format_efficiency(GIB, 0) == "—"


def test_format_efficiency_one_decimal_below_threshold():
    assert format_efficiency(GIB, 1800) == "2.0 GB/h"
    assert format_efficiency(int(2.5 * GIB), 3600) == "2.5 GB/h"


def test_format_efficiency_no_decimals_at_or_above_10():
    assert format_efficiency(12 * GIB, 3600) == "12 GB/h"
    assert format_efficiency(10 * GIB, 3600) == "10 GB/h"


# ---------------------------------------------------------------------------
# parse_size_to_bytes
# ---------------------------------------------------------------------------


def test_parse_size_to_bytes_units():
    assert parse_size_to_bytes("500 B") == 500
    assert parse_size_to_bytes("500 KB") == 500 * 1024
    assert parse_size_to_bytes("500 MB") == 500 * 1024**2
    assert parse_size_to_bytes("1.2 GB") == 1.2 * GIB
    assert parse_size_to_bytes("2 TB") == 2 * 1024**4


def test_parse_size_to_bytes_strips_estimate_prefix():
    assert parse_size_to_bytes("~1.2 GB") == parse_size_to_bytes("1.2 GB")


def test_parse_size_to_bytes_invalid_values_sort_last():
    assert parse_size_to_bytes("—") == math.inf
    assert parse_size_to_bytes("garbage") == math.inf
    assert parse_size_to_bytes("5 XB") == math.inf
    assert parse_size_to_bytes("1 2 GB") == math.inf


# ---------------------------------------------------------------------------
# parse_time_to_seconds
# ---------------------------------------------------------------------------


def test_parse_time_to_seconds_roundtrips_formatted_values():
    assert parse_time_to_seconds("2h 15m") == 8100
    assert parse_time_to_seconds("~~2h 15m") == 8100
    assert parse_time_to_seconds("~45m") == 2700
    assert parse_time_to_seconds("3h") == 10800


def test_parse_time_to_seconds_under_one_minute_is_30():
    assert parse_time_to_seconds("< 1m") == 30
    assert parse_time_to_seconds("~< 1m") == 30
    assert parse_time_to_seconds("~~< 1m") == 30


def test_parse_time_to_seconds_invalid_values_sort_last():
    assert parse_time_to_seconds("—") == math.inf
    assert parse_time_to_seconds("nonsense") == math.inf
    # Quirk (pinned): a parsed total of exactly zero is treated as unparseable.
    assert parse_time_to_seconds("0m") == math.inf


# ---------------------------------------------------------------------------
# parse_efficiency_to_value
# ---------------------------------------------------------------------------


def test_parse_efficiency_to_value():
    assert parse_efficiency_to_value("2.5 GB/h") == 2.5
    assert parse_efficiency_to_value("12 GB/h") == 12


def test_parse_efficiency_to_value_invalid_values_sort_last():
    assert parse_efficiency_to_value("—") == -math.inf
    assert parse_efficiency_to_value("5 MB/h") == -math.inf  # only GB/h is recognized
    assert parse_efficiency_to_value("junk") == -math.inf
    assert parse_efficiency_to_value("1 2 GB/h") == -math.inf
