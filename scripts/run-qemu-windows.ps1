# run-qemu-windows.ps1
# Lance QEMU sous Windows pour tester le kernel

$IsoPath = "C:\Users\Eric\Documents\Exo-OS\build\test-minimal.iso"

Write-Host "==========================================" -ForegroundColor Blue
Write-Host "  Test QEMU - Kernel Minimal" -ForegroundColor Blue
Write-Host "==========================================" -ForegroundColor Blue
Write-Host ""

# Chercher QEMU dans les emplacements communs
$QemuPaths = @(
    "C:\Program Files\qemu\qemu-system-x86_64.exe",
    "C:\Program Files (x86)\qemu\qemu-system-x86_64.exe",
    "C:\qemu\qemu-system-x86_64.exe"
)

$QemuExe = $null
foreach ($path in $QemuPaths) {
    if (Test-Path $path) {
        $QemuExe = $path
        break
    }
}

if (-not $QemuExe) {
    Write-Host "[INFO] QEMU n'est pas installé sous Windows" -ForegroundColor Yellow
    Write-Host "[INFO] Utilisation de QEMU via WSL..." -ForegroundColor Yellow
    Write-Host ""
    
    # Utiliser WSL avec timeout pour éviter de bloquer
    wsl bash -c "cd /mnt/c/Users/Eric/Documents/Exo-OS && timeout 10 qemu-system-x86_64 -cdrom build/test-minimal.iso -boot d -m 512M -nographic 2>&1"
} else {
    Write-Host "[OK] QEMU trouvé: $QemuExe" -ForegroundColor Green
    Write-Host "[INFO] Lancement de QEMU..." -ForegroundColor Cyan
    Write-Host "[INFO] Cherchez '!!' et 'ETST' en couleur à l'écran" -ForegroundColor Cyan
    Write-Host ""
    
    & $QemuExe -cdrom $IsoPath -boot d -m 512M
}

Write-Host ""
Write-Host "==========================================" -ForegroundColor Blue
Write-Host "  Test terminé" -ForegroundColor Blue
Write-Host "==========================================" -ForegroundColor Blue
