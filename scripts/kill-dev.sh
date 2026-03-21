#!/bin/bash
# kill-dev.sh — Kill ONLY dev instances of summongate.exe
# NEVER touches production (Program Files) or release builds.

powershell.exe -ExecutionPolicy Bypass -File "$(dirname "$0")/kill-dev.ps1"
