# Script PowerShell pour builder Exo-OS via WSL
# Utilisation: .\build-wsl.ps1

Write-Host "╔════════════════════════════════════════╗" -ForegroundColor Cyan
Write-Host "║   Build Exo-OS via WSL Ubuntu          ║" -ForegroundColor Cyan
Write-Host "╚════════════════════════════════════════╝" -ForegroundColor Cyan
Write-Host ""

# Vérifier que WSL est installé
try {
    $wslVersion = wsl --version 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "❌ WSL n'est pas installé ou n'est pas accessible" -ForegroundColor Red
        Write-Host ""
        Write-Host "Pour installer WSL:" -ForegroundColor Yellow
        Write-Host "  wsl --install -d Ubuntu" -ForegroundColor Yellow
        exit 1
    }
} catch {
    Write-Host "❌ Erreur lors de la vérification de WSL" -ForegroundColor Red
    exit 1
}

# Vérifier que Ubuntu est installé
$distros = wsl --list --quiet
if (-not ($distros -match "Ubuntu")) {
    Write-Host "❌ Ubuntu n'est pas installé dans WSL" -ForegroundColor Red
    Write-Host ""
    Write-Host "Pour installer Ubuntu:" -ForegroundColor Yellow
    Write-Host "  wsl --install -d Ubuntu" -ForegroundColor Yellow
    exit 1
}

Write-Host "✓ WSL Ubuntu détecté" -ForegroundColor Green
Write-Host ""

# Obtenir le chemin WSL du répertoire actuel
$currentPath = Get-Location
$wslPath = $currentPath.Path -replace '\\', '/' -replace 'C:', '/mnt/c'

Write-Host "📁 Répertoire: $currentPath" -ForegroundColor Cyan
Write-Host "📁 Path WSL: $wslPath" -ForegroundColor Cyan
Write-Host ""

# Menu de choix
Write-Host "Que voulez-vous faire?" -ForegroundColor Yellow
Write-Host "  [1] Installer les dépendances (setup-wsl.sh)" -ForegroundColor White
Write-Host "  [2] Compiler le projet (build-all.sh)" -ForegroundColor White
Write-Host "  [3] Compiler et tester dans QEMU (build + run)" -ForegroundColor White
Write-Host "  [4] Nettoyer les fichiers de build (clean.sh)" -ForegroundColor White
Write-Host "  [5] Ouvrir un shell WSL dans le projet" -ForegroundColor White
Write-Host ""

$choice = Read-Host "Votre choix [1-5]"

switch ($choice) {
    "1" {
        Write-Host ""
        Write-Host "🔧 Installation des dépendances..." -ForegroundColor Cyan
        wsl bash -c "cd '$wslPath' && ./scripts/setup-wsl.sh"
    }
    "2" {
        Write-Host ""
        Write-Host "🔨 Compilation du projet..." -ForegroundColor Cyan
        wsl bash -c "cd '$wslPath' && ./scripts/build-all.sh"
    }
    "3" {
        Write-Host ""
        Write-Host "🔨 Compilation..." -ForegroundColor Cyan
        wsl bash -c "cd '$wslPath' && ./scripts/build-all.sh"
        
        if ($LASTEXITCODE -eq 0) {
            Write-Host ""
            Write-Host "🚀 Lancement dans QEMU..." -ForegroundColor Cyan
            Write-Host "   (Appuyez sur Ctrl+A puis X pour quitter)" -ForegroundColor Yellow
            Write-Host ""
            wsl bash -c "cd '$wslPath' && ./scripts/run-qemu.sh"
        } else {
            Write-Host ""
            Write-Host "❌ La compilation a échoué" -ForegroundColor Red
        }
    }
    "4" {
        Write-Host ""
        Write-Host "🧹 Nettoyage..." -ForegroundColor Cyan
        wsl bash -c "cd '$wslPath' && ./scripts/clean.sh"
    }
    "5" {
        Write-Host ""
        Write-Host "🐧 Ouverture du shell WSL..." -ForegroundColor Cyan
        wsl bash -c "cd '$wslPath' && exec bash"
    }
    default {
        Write-Host ""
        Write-Host "❌ Choix invalide" -ForegroundColor Red
        exit 1
    }
}

Write-Host ""
if ($LASTEXITCODE -eq 0) {
    Write-Host "✓ Terminé avec succès!" -ForegroundColor Green
} else {
    Write-Host "❌ Une erreur s'est produite (code: $LASTEXITCODE)" -ForegroundColor Red
}
Write-Host ""
