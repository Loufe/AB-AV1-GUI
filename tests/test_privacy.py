# tests/test_privacy.py
"""Characterization tests for the pure anonymization functions in src/privacy.py.

PathPrivacyFilter is intentionally NOT tested here (its behavior is in flux).
"""

import re

import pytest
from src.privacy import anonymize_file, anonymize_folder, anonymize_path, set_anonymization_folders

FILE_PATTERN = re.compile(r"^file_[0-9a-f]{12}\.mp4$")
FOLDER_PATTERN = re.compile(r"^folder_[0-9a-f]{12}$")


@pytest.fixture(autouse=True)
def reset_configured_folders():
    """Keep the module-level input/output folder config from leaking between tests."""
    set_anonymization_folders(None, None)
    yield
    set_anonymization_folders(None, None)


# ---------------------------------------------------------------------------
# anonymize_file
# ---------------------------------------------------------------------------


def test_anonymize_file_shape_and_extension():
    result = anonymize_file("movie.mp4")
    assert FILE_PATTERN.match(result)
    assert result.endswith(".mp4")


def test_anonymize_file_is_deterministic():
    assert anonymize_file("movie.mp4") == anonymize_file("movie.mp4")


def test_anonymize_file_different_names_get_different_hashes():
    assert anonymize_file("movie.mp4") != anonymize_file("other.mp4")


def test_anonymize_file_hash_ignores_extension():
    # Hash is computed from the stem only; extension is carried over verbatim.
    mp4 = anonymize_file("movie.mp4")
    mkv = anonymize_file("movie.mkv")
    assert mp4.removesuffix(".mp4") == mkv.removesuffix(".mkv")


def test_anonymize_file_uses_basename_of_full_path():
    assert anonymize_file("/some/folder/movie.mp4") == anonymize_file("movie.mp4")


def test_anonymize_file_empty_string():
    assert anonymize_file("") == "file_unknown"


# ---------------------------------------------------------------------------
# anonymize_folder
# ---------------------------------------------------------------------------


def test_anonymize_folder_shape_and_determinism():
    result = anonymize_folder("/videos/library")
    assert FOLDER_PATTERN.match(result)
    assert anonymize_folder("/videos/library") == result


def test_anonymize_folder_different_paths_get_different_hashes():
    assert anonymize_folder("/videos/library") != anonymize_folder("/videos/other")


def test_anonymize_folder_normalizes_trailing_slash():
    assert anonymize_folder("/videos/library/") == anonymize_folder("/videos/library")


def test_anonymize_folder_empty_string():
    assert anonymize_folder("") == "[unknown]"


def test_configured_input_and_output_folders_get_labels():
    set_anonymization_folders("/videos/in", "/videos/out")

    assert anonymize_folder("/videos/in") == "[input_folder]"
    assert anonymize_folder("/videos/in/") == "[input_folder]"
    assert anonymize_folder("/videos/out") == "[output_folder]"
    # Subfolders and unrelated folders still hash.
    assert FOLDER_PATTERN.match(anonymize_folder("/videos/in/season1"))
    assert FOLDER_PATTERN.match(anonymize_folder("/videos/elsewhere"))


def test_clearing_configured_folders_restores_hashing():
    set_anonymization_folders("/videos/in", None)
    assert anonymize_folder("/videos/in") == "[input_folder]"

    set_anonymization_folders(None, None)
    assert FOLDER_PATTERN.match(anonymize_folder("/videos/in"))


# ---------------------------------------------------------------------------
# anonymize_path
# ---------------------------------------------------------------------------


def test_anonymize_path_combines_folder_and_file():
    result = anonymize_path("/videos/library/movie.mp4")
    folder_part, file_part = result.split("/")
    assert folder_part == anonymize_folder("/videos/library")
    assert file_part == anonymize_file("movie.mp4")


def test_anonymize_path_uses_input_folder_label():
    set_anonymization_folders("/videos/in", None)
    result = anonymize_path("/videos/in/movie.mp4")
    assert result == f"[input_folder]/{anonymize_file('movie.mp4')}"


def test_anonymize_path_empty_string():
    assert anonymize_path("") == "[unknown]/file_unknown"


def test_anonymize_path_bare_filename_gets_unknown_folder():
    # A bare filename has no dirname, which anonymize_folder maps to "[unknown]".
    result = anonymize_path("movie.mp4")
    assert result == f"[unknown]/{anonymize_file('movie.mp4')}"
