@echo off
chcp 65001 >nul
cd /d "%~dp0"
if not exist "字体预览.exe" (
  echo 缺少 字体预览.exe，正在编译...
  cargo build --release
  if errorlevel 1 (
    echo 编译失败，请检查 Rust 工具链。
    pause
    exit /b 1
  )
  copy /y "target\release\font_preview.exe" "字体预览.exe" >nul
)
start "" "字体预览.exe"
