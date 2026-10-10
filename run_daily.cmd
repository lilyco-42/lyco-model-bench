@echo off
rem 每日模型发现 + token 测速（Rust 版, Windows）。
rem 任务计划程序指向本文件；结果落 results\，日志追加到 results\daily.log。
setlocal
set "ROOT=%~dp0"
set "BIN=%ROOT%target\release\lyco-model-bench.exe"
if not exist "%ROOT%results" mkdir "%ROOT%results"
echo ==== %DATE% %TIME% ==== >> "%ROOT%results\daily.log"
rem 首次运行（或清过 target）时先编译 release 二进制。
if not exist "%BIN%" (
    pushd "%ROOT%"
    cargo build --release --locked >> "%ROOT%results\daily.log" 2>&1
    if errorlevel 1 ( popd & exit /b 1 )
    popd
)
"%BIN%" daily --provider both --reps 3 --max-tokens 512 --concurrency 6 --out "%ROOT%results" >> "%ROOT%results\daily.log" 2>&1
