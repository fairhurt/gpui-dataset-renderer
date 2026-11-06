# Dev loop: rebuilds and relaunches the app whenever .dev\trigger changes.
# Started by dev.cmd. Logs go to .dev\build.log / .dev\app-*.log.
Set-Location (Split-Path $PSScriptRoot -Parent)
$Host.UI.RawUI.WindowTitle = "gpui-dataset-renderer dev loop"
New-Item -ItemType Directory -Force .dev | Out-Null

if (-not $env:GPUI_FXC_PATH -or -not (Test-Path $env:GPUI_FXC_PATH)) {
    $env:GPUI_FXC_PATH = & "$PSScriptRoot\find-fxc.ps1"
}
if (-not $env:GPUI_FXC_PATH) {
    Write-Host "fxc.exe not found - install the Windows 11 SDK (Visual Studio Installer)." -ForegroundColor Red
    "FXC NOT FOUND" | Set-Content .dev\status
    Read-Host "Press Enter to exit"
    exit 1
}
$env:RUST_BACKTRACE = "1"
$exe = "target\release\gpui-dataset-renderer.exe"
Write-Host "fxc: $env:GPUI_FXC_PATH"
Write-Host "Watching .dev\trigger (close this window to stop)" -ForegroundColor Cyan

$last = "<start>"
while ($true) {
    $t = if (Test-Path .dev\trigger) { (Get-Content .dev\trigger -Raw).Trim() } else { "build" }
    if ($t -ne $last) {
        $last = $t
        $action = ($t -split '\s+')[0]
        $stamp = Get-Date -Format "HH:mm:ss"
        Get-Process gpui-dataset-renderer -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Milliseconds 300
        "running $t" | Set-Content .dev\status

        if ($action -eq "test") {
            Write-Host "[$stamp] cargo test ($t)" -ForegroundColor Yellow
            cmd /c "cargo test --release --color never > .dev\test.log 2>&1"
            $code = $LASTEXITCODE
            "EXIT $code" | Add-Content .dev\test.log
            "done $t exit=$code" | Set-Content .dev\status
            Write-Host "[$stamp] tests exit=$code"
            continue
        }

        Write-Host "[$stamp] cargo build --release ($t)" -ForegroundColor Yellow
        $sw = [Diagnostics.Stopwatch]::StartNew()
        cmd /c "cargo build --release --color never > .dev\build.log 2>&1"
        $code = $LASTEXITCODE
        "EXIT $code after $([int]$sw.Elapsed.TotalSeconds)s" | Add-Content .dev\build.log
        if ($code -eq 0) {
            Write-Host "[$stamp] build ok in $([int]$sw.Elapsed.TotalSeconds)s - launching" -ForegroundColor Green
            $appArgs = @()
            if ($action -eq "run" -and $t.Length -gt 4) { $appArgs = $t.Substring(4).Trim() -split '\s+' }
            if ($appArgs.Count -gt 0) {
                Start-Process -FilePath $exe -ArgumentList $appArgs -NoNewWindow -RedirectStandardError .dev\app-stderr.log -RedirectStandardOutput .dev\app-stdout.log
            } else {
                Start-Process -FilePath $exe -NoNewWindow -RedirectStandardError .dev\app-stderr.log -RedirectStandardOutput .dev\app-stdout.log
            }
        } else {
            Write-Host "[$stamp] build FAILED (see .dev\build.log)" -ForegroundColor Red
        }
        "done $t exit=$code" | Set-Content .dev\status
    }
    Start-Sleep -Milliseconds 700
}
