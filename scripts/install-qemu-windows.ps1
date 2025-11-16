# install-qemu-windows.ps1
# Script pour installer QEMU sous Windows via winget

Write-Host "Installation de QEMU pour Windows..." -ForegroundColor Cyan

# Vérifier si QEMU est déjà installé
$qemuPath = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
if ($qemuPath) {
    Write-Host "[OK] QEMU déjà installé: $($qemuPath.Source)" -ForegroundColor Green
    & qemu-system-x86_64 --version
    exit 0
}

# Installer via winget
Write-Host "Installation via winget..." -ForegroundColor Yellow
try {
    winget install QEMU.QEMU --accept-package-agreements --accept-source-agreements
    Write-Host "[OK] QEMU installé avec succès" -ForegroundColor Green
    
    # Winget ajoute QEMU au PATH mais nécessite de rafraîchir la session
    Write-Host "[INFO] Rafraîchissement du PATH..." -ForegroundColor Yellow
    $env:Path = [System.Environment]::GetEnvironmentVariable("Path","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("Path","User")
    
    # Vérifier l'installation
    $qemuPath = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
    if ($qemuPath) {
        Write-Host "[OK] QEMU trouvé: $($qemuPath.Source)" -ForegroundColor Green
        & qemu-system-x86_64 --version
    } else {
        Write-Host "[WARNING] QEMU installé mais non trouvé dans PATH" -ForegroundColor Yellow
        Write-Host "[INFO] Redémarrez PowerShell ou ajoutez C:\Program Files\qemu au PATH" -ForegroundColor Yellow
    }
} catch {
    Write-Host "[ERROR] Échec de l'installation: $_" -ForegroundColor Red
    Write-Host "[INFO] Installation manuelle: https://www.qemu.org/download/#windows" -ForegroundColor Yellow
    exit 1
}
