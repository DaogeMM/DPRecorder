@echo off
chcp 65001 >nul
rem 开发模式启动记录器：等价于在 DPR 目录里跑 cargo tauri dev。
cd /d "%~dp0DPR"
cargo tauri dev
pause