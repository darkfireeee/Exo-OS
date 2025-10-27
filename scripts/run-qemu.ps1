# run-qemu.ps1 - Script pour lancer Exo-OS dans QEMU

# Arrêter toute instance QEMU en cours
Stop-Process -Name "qemu-system-x86_64" -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

# Chemin vers l'image bootable
$KERNEL_IMAGE = "C:\Users\Eric\Documents\Exo-OS\target\x86_64-unknown-none\debug\bootimage-exo-kernel.bin"

# Vérifier que l'image existe
if (-not (Test-Path $KERNEL_IMAGE)) {
    Write-Host "ERREUR: Image bootable non trouvée à $KERNEL_IMAGE" -ForegroundColor Red
    Write-Host "Lancez d'abord: cargo bootimage --target ../x86_64-unknown-none.json" -ForegroundColor Yellow
    exit 1
}

Write-Host "Lancement d'Exo-OS dans QEMU..." -ForegroundColor Green
Write-Host "Image: $KERNEL_IMAGE" -ForegroundColor Cyan

# Lancer QEMU avec affichage graphique
& "C:\Program Files\qemu\qemu-system-x86_64.exe" `
    -drive "format=raw,file=$KERNEL_IMAGE" `
    -m 128M `
    -cpu qemu64 `
    -smp 4 `
    -no-reboot `
    -no-shutdown

Write-Host "`nQEMU terminé." -ForegroundColor Yellow
