# tests/test_utils.py
"""Tests for pure formatting helpers in src/utils.py."""

import pytest
from src.utils import format_crf


@pytest.mark.parametrize(
    ("crf", "expected"), [(23, "23"), (23.0, "23"), (23.25, "23.25"), (23.5, "23.5"), (68.75, "68.75"), (5, "5")]
)
def test_format_crf(crf, expected):
    assert format_crf(crf) == expected
