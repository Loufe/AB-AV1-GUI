@echo off
cd %~dp0

rem Check if Python is available
python --version >nul 2>&1
if %errorlevel% neq 0 (
    echo Python is not installed or not in PATH.
    echo.
    echo Please install Python 3.8+ from https://www.python.org/downloads/
    echo Make sure to check "Add Python to PATH" during installation.
    pause
    exit /b 1
)

echo Starting AV1 Video Converter...
python -m src.convert
if %errorlevel% neq 0 (
    echo.
    echo Application exited with an error.
    pause
)
