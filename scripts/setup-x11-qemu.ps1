# Installation et configuration X11 pour QEMU depuis WSL
# Permet d'afficher la fenêtre QEMU depuis WSL

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "  CONFIGURATION X11 POUR QEMU" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

Write-Host "Étape 1: Installer VcXsrv (serveur X11 pour Windows)" -ForegroundColor Yellow
Write-Host "  Télécharger depuis: https://sourceforge.net/projects/vcxsrv/" -ForegroundColor White
Write-Host ""

Write-Host "Étape 2: Configuration dans WSL" -ForegroundColor Yellow
$wslCommands = @"
# Dans WSL Ubuntu
export DISPLAY=:0
echo 'export DISPLAY=:0' >> ~/.bashrc
cd /mnt/c/Users/Eric/Documents/Exo-OS
qemu-system-x86_64 -cdrom build/exo-os-v2.iso -m 512M
"@
Write-Host $wslCommands -ForegroundColor Gray
Write-Host ""

Write-Host "Appuyez sur Entrée pour créer le script de configuration..." -ForegroundColor Yellow
Read-Host

# Créer script WSL
$scriptContent = @"
#!/bin/bash
export DISPLAY=:0
cd /mnt/c/Users/Eric/Documents/Exo-OS
qemu-system-x86_64 -cdrom build/exo-os-v2.iso -m 512M
"@

$scriptContent | Out-File -FilePath "C:\Users\Eric\Documents\Exo-OS\scripts\run-qemu-x11.sh" -Encoding utf8 -NoNewline

Write-Host "✅ Script créé: scripts/run-qemu-x11.sh" -ForegroundColor Green
Write-Host ""

Write-Host "INSTRUCTIONS:" -ForegroundColor Green
Write-Host "1. Installer VcXsrv: https://sourceforge.net/projects/vcxsrv/" -ForegroundColor White
Write-Host "2. Lancer VcXsrv (XLaunch) avec les options par défaut" -ForegroundColor White
Write-Host "3. Dans WSL, exécuter: chmod +x scripts/run-qemu-x11.sh" -ForegroundColor White
Write-Host "4. Dans WSL, exécuter: ./scripts/run-qemu-x11.sh" -ForegroundColor White
Write-Host ""
