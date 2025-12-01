# Build script for Exo-OS on Windows (invokes WSL)
# Usage: .\build.ps1

param(
    [switch]$Clean,
    [switch]$Release,
    [switch]$IsoOnly
)

$ErrorActionPreference = "Stop"

Write-Host "=== Exo-OS Build Script (Windows) ===" -ForegroundColor Cyan
Write-Host ""

# Check WSL installed
try {
    $wslCheck = wsl --status 2>$null
    if ($LASTEXITCODE -ne 0) {
        throw "WSL not accessible"
    }
} catch {
    Write-Host "Error: WSL not installed or not running" -ForegroundColor Red
    Write-Host "Install WSL with: wsl --install" -ForegroundColor Yellow
    exit 1
}

Write-Host "✓ WSL detected" -ForegroundColor Green
Write-Host ""

# Get Windows path and convert to WSL path
$ProjectPath = $PSScriptRoot
$WslPath = $ProjectPath -replace '\\', '/' -replace 'C:', '/mnt/c'
Write-Host "Project path: $ProjectPath" -ForegroundColor Gray
Write-Host "WSL path:     $WslPath" -ForegroundColor Gray
Write-Host ""

# Clean if requested
if ($Clean) {
    Write-Host "[Clean] Removing build artifacts..." -ForegroundColor Yellow
    wsl bash -c "cd '$WslPath' && rm -rf build/ target/"
    cargo clean --manifest-path kernel/Cargo.toml
    Write-Host "✓ Cleaned" -ForegroundColor Green
    Write-Host ""
}

# Build mode
$BuildMode = if ($Release) { "release" } else { "debug" }
Write-Host "Build mode: $BuildMode" -ForegroundColor Cyan
Write-Host ""

# Run build in WSL
Write-Host "[1/3] Building in WSL Ubuntu..." -ForegroundColor Blue
Write-Host ""

$buildCmd = "cd '$WslPath' && chmod +x build.sh scripts/*.sh && ./build.sh"
if ($Release) {
    $buildCmd += " --release"
}
if ($IsoOnly) {
    $buildCmd = "cd '$WslPath' && chmod +x scripts/make_iso.sh && ./scripts/make_iso.sh"
}

wsl bash -c $buildCmd

if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "Build failed!" -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "=== Build completed! ===" -ForegroundColor Green
Write-Host ""

# Show output files
if (Test-Path "build/kernel.bin") {
    $kernelSize = (Get-Item "build/kernel.bin").Length
    Write-Host "✓ Kernel binary: build/kernel.bin ($([math]::Round($kernelSize/1KB, 2)) KB)" -ForegroundColor Green
}

if (Test-Path "build/exo_os.iso") {
    $isoSize = (Get-Item "build/exo_os.iso").Length
    Write-Host "✓ Bootable ISO:  build/exo_os.iso ($([math]::Round($isoSize/1MB, 2)) MB)" -ForegroundColor Green
}

Write-Host ""
Write-Host "To test:" -ForegroundColor Cyan
Write-Host "  .\scripts\test_qemu.ps1" -ForegroundColor White
Write-Host "  or" -ForegroundColor Gray
Write-Host "  wsl ./scripts/test_qemu.sh" -ForegroundColor White
Write-Host ""
