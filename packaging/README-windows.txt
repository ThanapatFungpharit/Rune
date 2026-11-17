rune-tui for Windows (x86_64)
=============================

rune-tui is a terminal chat client for LINE. Run it in Windows Terminal (or
another terminal that supports colours and Unicode); the old console host
draws it poorly.

Installing
----------
install.ps1 (from the same release) installs, updates and removes it, and
checks the download's SHA-256 before anything is put in place:

    powershell -ExecutionPolicy Bypass -File install.ps1
    powershell -ExecutionPolicy Bypass -File install.ps1 -Archive rune-tui-x86_64-pc-windows-msvc.zip
    powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall

By default it goes to %LOCALAPPDATA%\Programs\rune\bin, which is added to
your user PATH (-NoModifyPath leaves PATH alone). Or copy bin\rune-tui.exe
anywhere yourself: it is a single program with no other files.

Where your data lives
---------------------
  %LOCALAPPDATA%\rune         login session, messages, ltsm.wasm, plugins
  %LOCALAPPDATA%\rune\cache   pictures and media opened
  %APPDATA%\rune              configuration (e.g. keys.json)

Installing, updating and uninstalling never touch these folders; delete them
yourself to remove everything.

ltsm.wasm
---------
rune-tui needs LINE's ltsm.wasm, which is not distributed with it. It is found
in Chrome, Edge, Brave, Chromium or Vivaldi when the LINE extension is
installed there, or put it in %LOCALAPPDATA%\rune\ltsm.wasm. See "Getting
ltsm.wasm" in README.md.

Optional programs it uses when they are on PATH: mpv (video and audio) and
git (plugins from Git).
