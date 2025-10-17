# Script de test direct avec QEMU pour Exo-OS
# Ce script teste le kernel compilé avec QEMU

$ErrorActionPreference = "Stop"

Write-Host "🚀 Test du Kernel Exo-OS" -ForegroundColor Cyan
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
Write-Host ""

# Compiler le kernel
Write-Host "[1/3] Compilation du kernel..." -ForegroundColor Yellow
Set-Location kernel
$buildOutput = cargo +nightly build --target "../x86_64-unknown-none.json" -Z build-std=core,alloc,compiler_builtins 2>&1
$exitCode = $LASTEXITCODE

if ($exitCode -ne 0) {
    Write-Host "❌ Échec de compilation!" -ForegroundColor Red
    $buildOutput | Select-String "error" | ForEach-Object { Write-Host $_ -ForegroundColor Red }
    exit 1
}

Write-Host "✅ Compilation réussie" -ForegroundColor Green
Set-Location ..

# Trouver QEMU
Write-Host ""
Write-Host "[2/3] Recherche de QEMU..." -ForegroundColor Yellow

$qemuExe = $null
$qemuCmd = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
if ($qemuCmd) {
    $qemuExe = $qemuCmd.Source
    Write-Host "✅ QEMU trouvé dans PATH: $qemuExe" -ForegroundColor Green
} else {
    $qemuStandard = "C:\Program Files\qemu\qemu-system-x86_64.exe"
    if (Test-Path $qemuStandard) {
        $qemuExe = $qemuStandard
        Write-Host "✅ QEMU trouvé: $qemuExe" -ForegroundColor Green
    } else {
        Write-Host "❌ QEMU non trouvé!" -ForegroundColor Red
        Write-Host ""
        Write-Host "Installer QEMU:" -ForegroundColor Yellow
        Write-Host "  • Chocolatey: choco install qemu" -ForegroundColor Gray
        Write-Host "  • Scoop: scoop install qemu" -ForegroundColor Gray
        Write-Host "  • Manuel: https://qemu.weilnetz.de/w64/" -ForegroundColor Gray
        exit 1
    }
}

# Note: Le kernel seul ne peut pas booter sans bootloader
# Cette section est préparée pour futurs tests
Write-Host ""
Write-Host "[3/3] Préparation du test..." -ForegroundColor Yellow
Write-Host "⚠️  Pour l'instant, le kernel nécessite un bootloader pour démarrer" -ForegroundColor Yellow
Write-Host ""
Write-Host "Prochaines étapes:" -ForegroundColor Cyan
Write-Host "  1. Créer une image ISO bootable" -ForegroundColor Gray
Write-Host "  2. Ou utiliser cargo bootimage avec un binaire" -ForegroundColor Gray
Write-Host "  3. Tester avec: $qemuExe -cdrom kernel.iso" -ForegroundColor Gray
Write-Host ""
Write-Host "Kernel compilé: kernel\target\x86_64-unknown-none\debug\libexo_kernel.a" -ForegroundColor Green
