#!/bin/bash
cd "$(dirname "$0")"

# Check if Python is available
if ! command -v python3 &> /dev/null; then
    echo "Python 3 is not installed or not in PATH."
    echo ""
    echo "Please install Python 3.8+ with Tkinter:"
    echo "  Ubuntu/Debian: sudo apt install python3 python3-tk"
    echo "  Fedora: sudo dnf install python3 python3-tkinter"
    echo "  macOS: brew install python-tk"
    exit 1
fi

echo "Starting AV1 Video Converter..."
python3 -m src.convert
