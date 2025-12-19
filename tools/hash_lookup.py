#!/usr/bin/env python3
"""
Lookup utility to find files by their anonymized BLAKE2b hash.

This tool scans a directory and computes hashes for each filename,
allowing you to reverse-lookup anonymized file references from logs
or history files.

Usage:
    python hash_lookup.py <hash_prefix> [directory]

Examples:
    python hash_lookup.py 7f3a9c2b         # Search current directory
    python hash_lookup.py 7f3a9c2b1e4d /path/to/videos
    python hash_lookup.py file_7f3a9c      # Prefix can include 'file_'
"""

import argparse
import hashlib
import os
import sys


def compute_file_hash(filename: str, length: int = 12) -> str:
    """Compute the BLAKE2b hash of a filename (matching the anonymizer).

    Args:
        filename: Just the filename (basename), not the full path
        length: Number of hex characters to return (default 12)

    Returns:
        Truncated hex hash string
    """
    # Normalize: lowercase on Windows
    normalized = filename.lower() if sys.platform == "win32" else filename

    # Hash the name without extension
    name_without_ext, _ = os.path.splitext(normalized)

    # Use BLAKE2b with 16-byte digest, truncate to specified length
    hash_bytes = hashlib.blake2b(name_without_ext.encode("utf-8"), digest_size=16).digest()
    return hash_bytes.hex()[:length]


def compute_folder_hash(folder_path: str, length: int = 12) -> str:
    """Compute the BLAKE2b hash of a folder path (matching the anonymizer).

    Args:
        folder_path: Full path to the folder
        length: Number of hex characters to return (default 12)

    Returns:
        Truncated hex hash string
    """
    # Normalize path
    normalized = os.path.normpath(os.path.abspath(folder_path))
    if sys.platform == "win32":
        normalized = normalized.lower()
    normalized = normalized.replace("\\", "/")

    # Compute hash
    hash_bytes = hashlib.blake2b(normalized.encode("utf-8"), digest_size=16).digest()
    return hash_bytes.hex()[:length]


def search_files(hash_prefix: str, directory: str, show_folders: bool = False) -> list[tuple[str, str, str]]:
    """Search for files matching a hash prefix.

    Args:
        hash_prefix: Hash prefix to search for (with or without 'file_' prefix)
        directory: Directory to scan recursively
        show_folders: Also compute and show folder hashes

    Returns:
        List of (hash, filepath, folder_hash) tuples for matches
    """
    # Strip 'file_' prefix if present
    search_hash = hash_prefix.lower()
    if search_hash.startswith("file_"):
        search_hash = search_hash[5:]

    matches = []

    for root, _dirs, files in os.walk(directory):
        folder_hash = compute_folder_hash(root) if show_folders else ""

        for filename in files:
            file_hash = compute_file_hash(filename)
            if file_hash.startswith(search_hash):
                filepath = os.path.join(root, filename)
                matches.append((file_hash, filepath, folder_hash))

    return matches


def search_folders(hash_prefix: str, directory: str) -> list[tuple[str, str]]:
    """Search for folders matching a hash prefix.

    Args:
        hash_prefix: Hash prefix to search for (with or without 'folder_' prefix)
        directory: Directory to scan recursively

    Returns:
        List of (hash, folderpath) tuples for matches
    """
    # Strip 'folder_' prefix if present
    search_hash = hash_prefix.lower()
    if search_hash.startswith("folder_"):
        search_hash = search_hash[7:]

    matches = []

    for root, _dirs, _files in os.walk(directory):
        folder_hash = compute_folder_hash(root)
        if folder_hash.startswith(search_hash):
            matches.append((folder_hash, root))

    return matches


def main():
    parser = argparse.ArgumentParser(
        description="Find files by their anonymized BLAKE2b hash",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  %(prog)s 7f3a9c2b              Search for files with hash starting with 7f3a9c2b
  %(prog)s file_7f3a9c /videos   Search in /videos directory
  %(prog)s --folder abc123       Search for folders with hash starting with abc123
  %(prog)s --list .              List all files with their hashes
        """,
    )
    parser.add_argument("hash_prefix", nargs="?", help="Hash prefix to search for (or use --list)")
    parser.add_argument("directory", nargs="?", default=".", help="Directory to scan (default: current)")
    parser.add_argument("--folder", "-f", action="store_true", help="Search for folder hashes instead of files")
    parser.add_argument("--list", "-l", action="store_true", help="List all files/folders with their hashes")
    parser.add_argument("--show-folders", "-s", action="store_true", help="Also show folder hashes for file matches")

    args = parser.parse_args()

    if not args.hash_prefix and not args.list:
        parser.error("Either provide a hash_prefix or use --list")

    directory = os.path.abspath(args.directory)
    if not os.path.isdir(directory):
        print(f"Error: '{directory}' is not a valid directory", file=sys.stderr)
        sys.exit(1)

    if args.list:
        # List mode: show all files/folders with their hashes
        print(f"Scanning: {directory}\n")

        if args.folder:
            print("Folder hashes:")
            print("-" * 60)
            for root, _dirs, _files in os.walk(directory):
                folder_hash = compute_folder_hash(root)
                print(f"folder_{folder_hash}  {root}")
        else:
            print("File hashes:")
            print("-" * 60)
            count = 0
            for root, _dirs, files in os.walk(directory):
                folder_hash = compute_folder_hash(root) if args.show_folders else ""
                for filename in files:
                    file_hash = compute_file_hash(filename)
                    filepath = os.path.join(root, filename)
                    _, ext = os.path.splitext(filename)
                    if args.show_folders:
                        print(f"folder_{folder_hash}/file_{file_hash}{ext}  {filepath}")
                    else:
                        print(f"file_{file_hash}{ext}  {filepath}")
                    count += 1
            print(f"\nTotal: {count} files")

    elif args.folder:
        # Folder search mode
        matches = search_folders(args.hash_prefix, directory)
        if matches:
            print(f"Found {len(matches)} matching folder(s):\n")
            for folder_hash, folderpath in matches:
                print(f"folder_{folder_hash}  {folderpath}")
        else:
            print(f"No folders found matching hash prefix '{args.hash_prefix}'")
            sys.exit(1)

    else:
        # File search mode
        matches = search_files(args.hash_prefix, directory, args.show_folders)
        if matches:
            print(f"Found {len(matches)} matching file(s):\n")
            for file_hash, filepath, folder_hash in matches:
                _, ext = os.path.splitext(filepath)
                if args.show_folders and folder_hash:
                    print(f"folder_{folder_hash}/file_{file_hash}{ext}")
                else:
                    print(f"file_{file_hash}{ext}")
                print(f"  -> {filepath}\n")
        else:
            print(f"No files found matching hash prefix '{args.hash_prefix}'")
            sys.exit(1)


if __name__ == "__main__":
    main()
