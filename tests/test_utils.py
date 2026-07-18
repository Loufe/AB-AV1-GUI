# tests/test_utils.py
"""Tests for pure formatting/parsing helpers in src/utils.py."""

import pytest
from src.utils import format_crf, parse_svt_av1_version


@pytest.mark.parametrize(
    ("crf", "expected"), [(23, "23"), (23.0, "23"), (23.25, "23.25"), (23.5, "23.5"), (68.75, "68.75"), (5, "5")]
)
def test_format_crf(crf, expected):
    assert format_crf(crf) == expected


@pytest.mark.parametrize(
    ("output", "expected"),
    [
        # Real banner captured from Gyan FFmpeg 8.1.2 (dev build suffix after patch version)
        ("Svt[info]: SVT [version]:\tSVT-AV1 Encoder Lib v4.1.0-259-gec17f8382", (4, 1)),
        ("Svt[info]: SVT [version]:\tSVT-AV1 Encoder Lib v3.1.0", (3, 1)),
        ("frame=1 fps=0.0 q=-0.0 size=0KiB time=00:00:00.00", None),
        ("", None),
    ],
)
def test_parse_svt_av1_version(output, expected):
    assert parse_svt_av1_version(output) == expected
