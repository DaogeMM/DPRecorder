@echo off
chcp 65001 >nul
rem 开发模式启动 DPSimulator：编译并运行 dps-main。
cd /d "%~dp0"
cargo run -p dps-main
pause