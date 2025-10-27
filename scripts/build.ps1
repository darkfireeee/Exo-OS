#!/usr/bin/env pwsh
# Script de build pour Exo-OS avec bootloader 0.11

Write-Host "╔════════════════════════════════════════╗" -ForegroundColor Cyan
Write-Host "║   Compilation d'Exo-OS Kernel v0.1.0  ║" -ForegroundColor Cyan
Write-Host "╚════════════════════════════════════════╝" -ForegroundColor Cyan
Write-Host ""

# Arrêter QEMU si en cours d'exécution
Write-Host "[1/4] Arrêt des instances QEMU..." -ForegroundColor Yellow
Stop-Process -Name "qemu-system-x86_64" -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

# Nettoyer les builds précédents
Write-Host "[2/4] Nettoyage des builds précédents..." -ForegroundColor Yellow
Set-Location "C:\Users\Eric\Documents\Exo-OS\kernel"
cargo clean

# Compiler le kernel avec bootloader 0.11
Write-Host "[3/4] Compilation du kernel..." -ForegroundColor Yellow
$env:RUST_BACKTRACE = "1"
cargo build --target ../x86_64-unknown-none.json -Z build-std=core,alloc,compiler_builtins

if ($LASTEXITCODE -ne 0) {
    Write-Host "❌ Erreur de compilation !" -ForegroundColor Red
    exit 1
}

# Créer l'image bootable avec bootimage
Write-Host "[4/4] Création de l'image bootable..." -ForegroundColor Yellow
cargo install bootimage --version 0.10.3 2>$null
cargo bootimage --target ../x86_64-unknown-none.json

if ($LASTEXITCODE -ne 0) {
    Write-Host "❌ Erreur lors de la création de l'image bootable !" -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "✅ Build réussi !" -ForegroundColor Green
Write-Host "   Image: C:\Users\Eric\Documents\Exo-OS\target\x86_64-unknown-none\debug\bootimage-exo-kernel.bin" -ForegroundColor Cyan
Write-Host ""
Write-Host "Pour tester: .\run-qemu.ps1" -ForegroundColor Yellow
