#!/usr/bin/env bash
# Fetch the pinned ffmpeg sidecar (gyan essentials, GPL: libx264).
set -euo pipefail
cd "$(dirname "$0")"
curl -L -C - -o /tmp/ff.zip https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip
unzip -o -j /tmp/ff.zip 'ffmpeg-*/bin/ffmpeg.exe' -d /tmp/sr-ff
cp /tmp/sr-ff/ffmpeg.exe ffmpeg-x86_64-pc-windows-msvc.exe
echo "ok: $(du -h ffmpeg-x86_64-pc-windows-msvc.exe | cut -f1)"
