@echo off
echo Starting AV1 Video Converter...
cd %~dp0
python main.py
if %errorlevel% neq 0 (
    echo Error launching the application. Please ensure Python is installed.
    pause
)
