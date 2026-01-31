@echo off
REM Build script wrapper for pico-scoreboard
REM Usage: build [flash|run] [options]

python "%~dp0build.py" %*
