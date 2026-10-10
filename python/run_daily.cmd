@echo off
rem Daily model discovery + token-speed benchmark (Windows).
setlocal
set PYTHONIOENCODING=utf-8
if not exist "%~dp0results" mkdir "%~dp0results"
echo ==== %DATE% %TIME% ==== >> "%~dp0results\daily.log"
python "%~dp0bench.py" --provider both --reps 3 --max-tokens 512 --concurrency 6 >> "%~dp0results\daily.log" 2>&1
