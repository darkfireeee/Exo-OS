# run-qemu-serial.ps1 - Script pour lancer Exo-OS dans QEMU avec sortie serial

# Arrêter toute instance QEMU en cours
Stop-Process -Name "qemu-system-x86_64" -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

# Chemin vers l'image bootable
$KERNEL_IMAGE = "C:\Users\Eric\Documents\Exo-OS\target\x86_64-unknown-none\debug\bootimage-exo-kernel.bin"
$SERIAL_LOG = "C:\Users\Eric\Documents\Exo-OS\serial-output.txt"

# Supprimer l'ancien log
if (Test-Path $SERIAL_LOG) {
    Remove-Item $SERIAL_LOG
}

# Vérifier que l'image existe
if (-not (Test-Path $KERNEL_IMAGE)) {
    Write-Host "ERREUR: Image bootable non trouvée à $KERNEL_IMAGE" -ForegroundColor Red
    Write-Host "Lancez d'abord: cargo bootimage --target ../x86_64-unknown-none.json" -ForegroundColor Yellow
    exit 1
}

Write-Host "Lancement d'Exo-OS dans QEMU (mode headless avec serial log)..." -ForegroundColor Green
Write-Host "Image: $KERNEL_IMAGE" -ForegroundColor Cyan
Write-Host "Serial log: $SERIAL_LOG" -ForegroundColor Cyan
Write-Host "`nAppuyez sur Ctrl+C pour arrêter QEMU`n" -ForegroundColor Yellow

# Lancer QEMU en mode headless avec serial sur fichier
$qemuProcess = Start-Process -FilePath "C:\Program Files\qemu\qemu-system-x86_64.exe" `
    -ArgumentList "-drive","format=raw,file=$KERNEL_IMAGE","-m","128M","-cpu","qemu64","-smp","4","-serial","file:$SERIAL_LOG","-nographic","-no-reboot","-no-shutdown" `
    -PassThru -NoNewWindow

Write-Host "QEMU PID: $($qemuProcess.Id)" -ForegroundColor Cyan

# Attendre un peu pour que le kernel démarre
Start-Sleep -Seconds 2

# Afficher le contenu du serial log
if (Test-Path $SERIAL_LOG) {
    Write-Host "`n========== SORTIE SERIAL ==========" -ForegroundColor Green
    Get-Content $SERIAL_LOG
    Write-Host "===================================`n" -ForegroundColor Green
    
    Write-Host "Surveillance du fichier log (Ctrl+C pour arrêter)..." -ForegroundColor Yellow
    
    # Surveiller le fichier en temps réel
    Get-Content $SERIAL_LOG -Wait -Tail 10
} else {
    Write-Host "Aucune sortie serial détectée." -ForegroundColor Red
}

Write-Host "`nQEMU terminé." -ForegroundColor Yellow
