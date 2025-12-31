# A Python AB-AV1 GUI

A lean, cross-platform [Tkinter](https://docs.python.org/3/library/tkinter.html)-based GUI for intelligently reducing the footprint of your video library by converting to [AV1](https://aomedia.org/av1/), with some bells & whistles.

## Premise

The AV1 video codec offers incredible gains for most video types vs standard [x264](https://www.videolan.org/developers/x264.html) and [x265](https://www.videolan.org/developers/x265.html) encoded files. However, letting [FFmpeg](https://ffmpeg.org/) rip through a project without tweaking parameters on a per-video basis produces terrible results. 

This tool mainly acts as a wrapper for the incredible cli tool [ab-av1](https://github.com/alexheretic/ab-av1), which randomly samples each video with varying parameters, efficiently discovering the right parameters to achieve maximum filesize reduction for your configured visual fidelity score (defaults to 95%). Ab-av1's magic is using the [VMAF](https://github.com/Netflix/vmaf) algorithm from Netflix for analyzing visual fidelity based on metrics which mirror human perception rather than statistical similarity. The tool then runs the conversion for the set parameters with FFmpeg.

## Features

- **VMAF-based quality targeting:** targets visual quality (_default: **95**_) instead of guessing bitrates
- **Queue-based workflow:** add files or folders, preview estimates, process sequentially
- **Private, secure, safe:** no pip packages, no telemetry, optional anonymization of history/logs

## Usage

- Install [Python](https://www.python.org/) 3.8+ ([Ensure your install includes Tkinter](https://stackoverflow.com/questions/76105218/why-does-tkinter-or-turtle-seem-to-be-missing-or-broken-shouldnt-it-be-part), included by default on Windows and MacOS).
- **Optional:** [FFmpeg](https://ffmpeg.org/) with libsvtav1 and [ab-av1](https://github.com/alexheretic/ab-av1/releases) may be pre-installed system-wide and available in PATH. Or download portable binaries in-app.


On Windows, double-click `convert.bat`. On Linux/macOS, run `./convert.sh`.

The app will prompt to download FFmpeg and ab-av1 if not found.

## Notes

- Tested on Windows. Designed for cross-platform but Linux/macOS are untested.
- **Media servers**: Be thoughtful about support for AV1 decoding in devices you want to watch video on. Old phones, PCs, streamers, and smart TVs may not support it, adding a high computational burden for transcoding on the server.

### Third-Party Software

- [FFmpeg](https://ffmpeg.org/) (LGPL 2.1+) — video encoding
- [ab-av1](https://github.com/alexheretic/ab-av1) (MIT) — VMAF optimization

### License

GPL-3.0 — see [LICENSE](LICENSE)

## Contributing & AI Disclosure

This tool has been developed with AI assistance under human guidance and code review.

Any contributions are welcome, but please note adding new dependencies is discouraged, and PRs should have already passed linting (ruff) and type checking (ty) without hacky exceptions.
