#!/usr/bin/env pwsh
# run.ps1 - Compile et lance Exo-OS dans QEMU
# Usage: .\scripts\run.ps1 [-Release] [-NoBuild]

param(
    [switch]$Release,
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"

Write-Host ""
Write-Host "═══════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host "  EXO-OS - Build & Run" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host ""

# Build if needed
if (-not $NoBuild) {
    Write-Host "→ Compilation..." -ForegroundColor Yellow
    
    $buildArgs = @()
    if ($Release) {
        $buildArgs += "-Release"
    }
    
    & "$PSScriptRoot\build_native.ps1" @buildArgs
    
    if ($LASTEXITCODE -ne 0) {
        Write-Host "✗ Erreur de compilation" -ForegroundColor Red
        exit 1
    }
}

# Check if kernel.bin exists
if (-not (Test-Path "build\kernel.bin")) {
    Write-Host "✗ build\kernel.bin introuvable" -ForegroundColor Red
    Write-Host "  Compilez d'abord avec: .\scripts\build_native.ps1" -ForegroundColor Yellow
    exit 1
}

# Run in QEMU
Write-Host ""
Write-Host "→ Lancement dans QEMU..." -ForegroundColor Yellow
Write-Host "  (Fermez la fenêtre QEMU pour terminer)" -ForegroundColor Gray
Write-Host ""

# Find QEMU
$qemuPaths = @(
    "qemu-system-x86_64",
    "C:\Program Files\qemu\qemu-system-x86_64.exe",
    "$env:ProgramFiles\qemu\qemu-system-x86_64.exe",
    "$env:LOCALAPPDATA\Programs\QEMU\qemu-system-x86_64.exe"
)

$qemu = $null
foreach ($path in $qemuPaths) {
    if (Get-Command $path -ErrorAction SilentlyContinue) {
        $qemu = $path
        break
    }
    if (Test-Path $path) {
        $qemu = $path
        break
    }
}

if (-not $qemu) {
    Write-Host "✗ QEMU non trouvé!" -ForegroundColor Red
    Write-Host "  Installez QEMU: https://www.qemu.org/download/#windows" -ForegroundColor Yellow
    Write-Host "  Ou via Chocolatey: choco install qemu" -ForegroundColor Yellow
    exit 1
}

Write-Host "QEMU: $qemu" -ForegroundColor Gray

# Clean previous logs
Remove-Item -Path "serial.log" -Force -ErrorAction SilentlyContinue
Remove-Item -Path "qemu.log" -Force -ErrorAction SilentlyContinue

# Run QEMU directly with kernel.bin (multiboot)
& $qemu `
    -kernel build\kernel.bin `
    -m 512M `
    -serial file:serial.log `
    -no-reboot `
    -no-shutdown `
    -d cpu_reset,guest_errors `
    -D qemu.log `
    -vga std

$qemuExitCode = $LASTEXITCODE

Write-Host ""
Write-Host "═══════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host "  Test terminé (QEMU exit code: $qemuExitCode)" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host ""

# Show serial output
if (Test-Path "serial.log") {
    $content = Get-Content "serial.log" -Raw
    if ($content) {
        Write-Host "Sortie série (serial.log):" -ForegroundColor Yellow
        Write-Host "─────────────────────────────────────────────────────────" -ForegroundColor Gray
        Write-Host $content
        Write-Host "─────────────────────────────────────────────────────────" -ForegroundColor Gray
    } else {
        Write-Host "Aucune sortie série enregistrée" -ForegroundColor Gray
    }
} else {
    Write-Host "Pas de fichier serial.log créé" -ForegroundColor Gray
}

Write-Host ""
Write-Host "Fichiers de log:" -ForegroundColor Cyan
Write-Host "  serial.log - Sortie série du kernel" -ForegroundColor White
Write-Host "  qemu.log   - Debug QEMU" -ForegroundColor White
Write-Host ""
