# Script de test Exo-OS avec Hyper-V
# Usage: Exécuter ce script en tant qu'administrateur

$VMName = "Exo-OS-Test"
$ISOPath = "C:\Users\Eric\Documents\Exo-OS\build\exo-os-v2.iso"

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  EXO-OS PHASE 8 - TEST BOOT HYPER-V  " -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# Vérifier si Hyper-V est disponible
$hypervFeature = Get-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V-All -ErrorAction SilentlyContinue
if ($null -eq $hypervFeature -or $hypervFeature.State -ne "Enabled") {
    Write-Host "⚠️  Hyper-V n'est pas activé sur ce système." -ForegroundColor Yellow
    Write-Host ""
    Write-Host "Options alternatives:" -ForegroundColor Yellow
    Write-Host "  1. Activer Hyper-V (nécessite redémarrage):" -ForegroundColor White
    Write-Host "     Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All" -ForegroundColor Gray
    Write-Host ""
    Write-Host "  2. Utiliser VirtualBox à la place:" -ForegroundColor White
    Write-Host "     - Télécharger: https://www.virtualbox.org/wiki/Downloads" -ForegroundColor Gray
    Write-Host "     - Créer VM: Linux Other (64-bit), 512 MB RAM" -ForegroundColor Gray
    Write-Host "     - Attacher ISO: $ISOPath" -ForegroundColor Gray
    Write-Host ""
    Write-Host "  3. Tester avec QEMU + X11 (nécessite configuration):" -ForegroundColor White
    Write-Host "     - Voir: Docs/MANUAL_TEST_INSTRUCTIONS.md" -ForegroundColor Gray
    Write-Host ""
    exit 1
}

# Vérifier si l'ISO existe
if (-not (Test-Path $ISOPath)) {
    Write-Host "❌ ISO introuvable: $ISOPath" -ForegroundColor Red
    Write-Host ""
    Write-Host "Veuillez compiler d'abord:" -ForegroundColor Yellow
    Write-Host "  wsl bash -c 'cd /mnt/c/Users/Eric/Documents/Exo-OS && ./scripts/build-iso.sh'" -ForegroundColor Gray
    Write-Host ""
    exit 1
}

Write-Host "✅ ISO trouvée: $(Get-Item $ISOPath | Select-Object -ExpandProperty Length | ForEach-Object { [math]::Round($_ / 1MB, 1) }) MB" -ForegroundColor Green
Write-Host ""

# Supprimer la VM existante si elle existe
$existingVM = Get-VM -Name $VMName -ErrorAction SilentlyContinue
if ($existingVM) {
    Write-Host "🗑️  Suppression de la VM existante..." -ForegroundColor Yellow
    if ($existingVM.State -eq "Running") {
        Stop-VM -Name $VMName -Force
    }
    Remove-VM -Name $VMName -Force
    Write-Host "✅ VM supprimée" -ForegroundColor Green
}

# Créer la nouvelle VM
Write-Host "🔧 Création de la VM '$VMName'..." -ForegroundColor Cyan
try {
    New-VM -Name $VMName -MemoryStartupBytes 512MB -Generation 1 -NoVHD | Out-Null
    Write-Host "✅ VM créée (512 MB RAM, Génération 1)" -ForegroundColor Green
} catch {
    Write-Host "❌ Erreur lors de la création de la VM: $_" -ForegroundColor Red
    exit 1
}

# Ajouter un lecteur DVD et attacher l'ISO
Write-Host "💿 Attachement de l'ISO..." -ForegroundColor Cyan
try {
    Add-VMDvdDrive -VMName $VMName -Path $ISOPath
    Write-Host "✅ ISO attachée" -ForegroundColor Green
} catch {
    Write-Host "❌ Erreur lors de l'attachement de l'ISO: $_" -ForegroundColor Red
    Remove-VM -Name $VMName -Force
    exit 1
}

# Configurer le boot
Set-VMFirmware -VMName $VMName -FirstBootDevice (Get-VMDvdDrive -VMName $VMName)

Write-Host ""
Write-Host "========================================" -ForegroundColor Green
Write-Host "  VM PRÊTE - DÉMARRAGE..." -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Green
Write-Host ""

Write-Host "📋 CE QUE VOUS DEVEZ OBSERVER:" -ForegroundColor Cyan
Write-Host ""
Write-Host "  1️⃣  MENU GRUB (devrait afficher):" -ForegroundColor Yellow
Write-Host "     'Exo-OS Kernel v0.2.0-PHASE8-BOOT'" -ForegroundColor White
Write-Host "     (PAS v0.1.0 !)" -ForegroundColor Gray
Write-Host ""
Write-Host "  2️⃣  MARQUEURS DEBUG (coin supérieur gauche):" -ForegroundColor Yellow
Write-Host "     AA BB PP 64 4 S C XXXXXXX..." -ForegroundColor White
Write-Host ""
Write-Host "     AA (blanc/rouge) = Entrée 32-bit OK" -ForegroundColor Gray
Write-Host "     BB (vert)        = Stack configuré" -ForegroundColor Gray
Write-Host "     PP (bleu)        = CPU 64-bit OK" -ForegroundColor Gray
Write-Host "     64 (blanc/rouge) = Mode 64-bit actif" -ForegroundColor Gray
Write-Host "     4  (vert)        = Segments chargés" -ForegroundColor Gray
Write-Host "     S  (bleu)        = Stack 64-bit OK" -ForegroundColor Gray
Write-Host "     C  (jaune)       = Avant rust_main" -ForegroundColor Gray
Write-Host "     XXX... (ligne verte) = rust_main exécuté ✅" -ForegroundColor Gray
Write-Host ""
Write-Host "  3️⃣  PAS D'ERREUR 'address is out of range'" -ForegroundColor Yellow
Write-Host ""

Write-Host "🚀 Démarrage de la VM et ouverture de la console..." -ForegroundColor Green
Write-Host ""

# Démarrer la VM
Start-VM -Name $VMName

# Ouvrir la console de connexion
Start-Sleep -Seconds 2
vmconnect.exe localhost $VMName

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  CONSOLE OUVERTE" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "📸 Merci de:" -ForegroundColor Yellow
Write-Host "  1. Prendre un screenshot du menu GRUB" -ForegroundColor White
Write-Host "  2. Prendre un screenshot des marqueurs (premiers caractères)" -ForegroundColor White
Write-Host "  3. Reporter ce que vous voyez" -ForegroundColor White
Write-Host ""
Write-Host "Pour arrêter la VM:" -ForegroundColor Gray
Write-Host "  Stop-VM -Name '$VMName' -Force" -ForegroundColor Gray
Write-Host ""
Write-Host "Pour supprimer la VM:" -ForegroundColor Gray
Write-Host "  Remove-VM -Name '$VMName' -Force" -ForegroundColor Gray
Write-Host ""
