@echo off
echo Starting AV1 Video Converter...
cd %~dp0
rem Run the 'convert' module within the 'src' package
python -m src.convert
if %errorlevel% neq 0 (
    echo Error launching the application via 'python -m src.convert'. Please ensure Python is installed and dependencies are met.
    pause
)