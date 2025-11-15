# Script pour désactiver Core Isolation (Memory Integrity)
# Exécuter en tant qu'administrateur

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  DÉSACTIVATION CORE ISOLATION          " -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

Write-Host "Étape 1: Désactivation de Memory Integrity..." -ForegroundColor Yellow
Write-Host ""

# Ouvrir les paramètres Windows Securité
Write-Host "ACTIONS MANUELLES REQUISES:" -ForegroundColor Green
Write-Host "1. Appuyez sur Windows + I (Paramètres)" -ForegroundColor White
Write-Host "2. Allez dans 'Confidentialité et sécurité'" -ForegroundColor White
Write-Host "3. Cliquez sur 'Sécurité Windows'" -ForegroundColor White
Write-Host "4. Cliquez sur 'Sécurité des appareils'" -ForegroundColor White
Write-Host "5. Sous 'Isolation du noyau', cliquez sur 'Détails'" -ForegroundColor White
Write-Host "6. DÉSACTIVEZ 'Intégrité de la mémoire'" -ForegroundColor Red
Write-Host "7. REDÉMARREZ l'ordinateur" -ForegroundColor Red
Write-Host ""

Write-Host "Appuyez sur Entrée pour ouvrir les Paramètres Windows..." -ForegroundColor Yellow
Read-Host

Start-Process "ms-settings:windowsdefender"

Write-Host ""
Write-Host "Après le redémarrage:" -ForegroundColor Green
Write-Host "  - VirtualBox pourra utiliser VT-x" -ForegroundColor White
Write-Host "  - Les performances seront BEAUCOUP meilleures" -ForegroundColor White
Write-Host "  - Le kernel devrait booter correctement" -ForegroundColor White
Write-Host ""
