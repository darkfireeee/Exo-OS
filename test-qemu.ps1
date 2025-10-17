# Script de test pour Exo-OS avec QEMU
# Usage: .\test-qemu.ps1

Write-Host "🚀 Compilation du kernel Exo-OS..." -ForegroundColor Cyan

# Compiler le kernel
Set-Location kernel
$buildResult = cargo +nightly build --target "../x86_64-unknown-none.json" -Z build-std=core,alloc,compiler_builtins 2>&1
$exitCode = $LASTEXITCODE

if ($exitCode -ne 0) {
    Write-Host "❌ Échec de compilation!" -ForegroundColor Red
    Write-Host $buildResult
    exit 1
}

Write-Host "✅ Compilation réussie!" -ForegroundColor Green
Set-Location ..

# Vérifier si QEMU est installé
$qemuPath = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue

if (-not $qemuPath) {
    Write-Host "⚠️  QEMU non trouvé!" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "Pour installer QEMU sur Windows:" -ForegroundColor White
    Write-Host "  1. Télécharger depuis: https://qemu.weilnetz.de/w64/" -ForegroundColor Gray
    Write-Host "  2. Ou via Chocolatey: choco install qemu" -ForegroundColor Gray
    Write-Host "  3. Ou via Scoop: scoop install qemu" -ForegroundColor Gray
    Write-Host ""
    Write-Host "Kernel compilé disponible ici:" -ForegroundColor White
    Write-Host "  kernel\target\x86_64-unknown-none\debug\libexo_kernel.a" -ForegroundColor Gray
    exit 0
}

Write-Host "🖥️  Lancement de QEMU..." -ForegroundColor Cyan
Write-Host ""
Write-Host "QEMU Options:" -ForegroundColor White
Write-Host "  - CPU: 4 cores" -ForegroundColor Gray
Write-Host "  - RAM: 256 MB" -ForegroundColor Gray
Write-Host "  - Serial: stdio" -ForegroundColor Gray
Write-Host "  - Display: none (headless)" -ForegroundColor Gray
Write-Host ""
Write-Host "Pour quitter QEMU: Ctrl+A puis X" -ForegroundColor Yellow
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
Write-Host ""

# Note: Pour l'instant, on ne peut pas booter directement sans bootloader
# Il faut d'abord créer une image bootable
Write-Host "⚠️  Image bootable non créée (bootloader requis)" -ForegroundColor Yellow
Write-Host ""
Write-Host "Options pour créer une image bootable:" -ForegroundColor White
Write-Host "  1. Utiliser bootimage: cargo install bootimage" -ForegroundColor Gray
Write-Host "  2. Utiliser GRUB avec une ISO" -ForegroundColor Gray
Write-Host "  3. Utiliser multiboot2 avec un stub" -ForegroundColor Gray
Write-Host ""
Write-Host "Voulez-vous installer bootimage pour tester? (o/n)" -ForegroundColor Cyan
$response = Read-Host

if ($response -eq 'o' -or $response -eq 'O' -or $response -eq 'y' -or $response -eq 'Y') {
    Write-Host "📦 Installation de bootimage..." -ForegroundColor Cyan
    cargo install bootimage
    
    if ($LASTEXITCODE -eq 0) {
        Write-Host "✅ bootimage installé!" -ForegroundColor Green
        Write-Host ""
        Write-Host "Ajoutez cette dépendance à kernel/Cargo.toml:" -ForegroundColor White
        Write-Host "[dependencies]" -ForegroundColor Gray
        Write-Host "bootloader = { version = `"0.9`", features = [`"map_physical_memory`"] }" -ForegroundColor Gray
    }
}
