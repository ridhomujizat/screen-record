# Binaries (Tauri sidecars)

`ffmpeg-x86_64-pc-windows-msvc.exe` — GPL build with libx264 (gyan.dev
essentials), bundled next to the app exe by `bundle.externalBin`.
Not committed (103 MB) — fetch it with:

```sh
./download-ffmpeg.sh   # or: bash download-ffmpeg.sh
```

Windows PowerShell equivalent:

```powershell
Invoke-WebRequest https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip -OutFile ff.zip
Expand-Archive ff.zip; Copy-Item ff\ffmpeg-*-essentials_build\bin\ffmpeg.exe .\ffmpeg-x86_64-pc-windows-msvc.exe
```
