# Script PowerShell pour tester Exo-OS dans QEMU
# Usage: .\test_qemu.ps1

$ErrorActionPreference = "Stop"

Write-Host "=== Test Exo-OS dans QEMU ===" -ForegroundColor Cyan
Write-Host ""

# Vérifier que l'ISO existe
if (-not (Test-Path "build\exo_os.iso")) {
    Write-Host "Erreur: build\exo_os.iso introuvable" -ForegroundColor Red
    Write-Host "Compilez d'abord sous WSL avec: wsl bash -c 'cd /mnt/c/Users/Eric/Documents/Exo-OS && ./build.sh && ./make_iso.sh'" -ForegroundColor Yellow
    exit 1
}

Write-Host "ISO trouvée: build\exo_os.iso" -ForegroundColor Green
Write-Host "Lancement de QEMU..." -ForegroundColor Cyan
Write-Host ""
Write-Host "Fermez la fenêtre QEMU pour terminer." -ForegroundColor Yellow
Write-Host ""

# Essayer différents chemins pour QEMU
$qemu_paths = @(
    "qemu-system-x86_64.exe",
    "C:\Program Files\qemu\qemu-system-x86_64.exe",
    "C:\Program Files (x86)\qemu\qemu-system-x86_64.exe"
)

$qemu = $null
foreach ($path in $qemu_paths) {
    if (Get-Command $path -ErrorAction SilentlyContinue) {
        $qemu = $path
        break
    }
}

if ($null -eq $qemu) {
    Write-Host "QEMU non trouvé. Tentative via WSL..." -ForegroundColor Yellow
    Write-Host ""
    
    # Lancer via WSL si QEMU n'est pas sur Windows
    wsl bash -c "cd /mnt/c/Users/Eric/Documents/Exo-OS && DISPLAY=:0 scripts/test_qemu.sh"
} else {
    Write-Host "QEMU trouvé: $qemu" -ForegroundColor Green
    Write-Host ""
    Write-Host "=== Traces de boot (stdout) ===" -ForegroundColor Cyan
    Write-Host ""
    
    # Lancer QEMU depuis Windows avec traces
    & $qemu `
        -cdrom build\exo_os.iso `
        -m 128M `
        -serial stdio `
        -no-reboot `
        -no-shutdown `
        -d cpu_reset
}

Write-Host ""
Write-Host "=== Test terminé ===" -ForegroundColor Cyan
