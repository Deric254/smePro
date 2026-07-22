#!/usr/bin/env bash
# Run this to install everything needed (first run only) and launch
# SME Pro in dev mode.
#
# macOS: right-click this file -> Open (first time only, to bypass the
# "unidentified developer" warning), or run `./run.sh` in Terminal.
# Linux: `./run.sh` in a terminal, or double-click if your file manager
# is set to run executable text files.

cd "$(dirname "$0")"
chmod +x scripts/setup.sh
./scripts/setup.sh --dev
