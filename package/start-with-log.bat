@echo off
rem Start layout-switcher with logging of typed words (layout-switcher.log next to the program).
rem If it is already running (tray icon near the clock), it is stopped first.
cd /d "%~dp0"
layout-switcher.exe --exit
timeout /t 2 /nobreak >nul
start "" layout-switcher.exe --log-words
