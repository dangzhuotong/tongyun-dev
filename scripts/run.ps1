$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location (Join-Path $ScriptDir "..")

Write-Host "=== 正在启动 tongyun-dev 本地控制台 ===" -ForegroundColor Cyan
cargo run -p tongyun-dev-cli -- $args
