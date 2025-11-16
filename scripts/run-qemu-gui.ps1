# run-qemu-gui.ps1
# Script PowerShell pour lancer QEMU en mode GUI sous Windows
# Utilise QEMU natif Windows pour l'interface graphique

param(
    [string]$IsoPath = "build\exo-os.iso",
    [int]$Memory = 512,
    [int]$Timeout = 0  # 0 = pas de timeout
)

$ErrorActionPreference = "Stop"

# Couleurs
function Write-Info { Write-Host "[INFO] $args" -ForegroundColor Cyan }
function Write-Success { Write-Host "[SUCCESS] $args" -ForegroundColor Green }
function Write-Error { Write-Host "[ERROR] $args" -ForegroundColor Red }

Write-Host "==========================================" -ForegroundColor Blue
Write-Host "  Exo-OS QEMU GUI Launcher (Windows)" -ForegroundColor Blue
Write-Host "==========================================" -ForegroundColor Blue

# Vérifier que l'ISO existe
if (-not (Test-Path $IsoPath)) {
    Write-Error "ISO introuvable: $IsoPath"
    Write-Info "Exécutez d'abord: .\scripts\rebuild-iso-phase1.ps1"
    exit 1
}

Write-Info "ISO: $IsoPath"
$isoFullPath = (Resolve-Path $IsoPath).Path
Write-Info "Chemin complet: $isoFullPath"
try {
    $hash = (Get-FileHash $isoFullPath -Algorithm SHA256).Hash
    Write-Info "SHA-256: $hash"
    $size = (Get-Item $isoFullPath).Length
    Write-Info ("Taille: {0} bytes" -f $size)
} catch {}

# Vérifier que QEMU est installé (Windows natif ou WSL)
$qemu = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
if (-not $qemu) {
    # Essayer via WSL
    $wslQemu = wsl which qemu-system-x86_64 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Error "QEMU non installé (ni Windows ni WSL)"
        Write-Info "Windows: Exécutez .\scripts\install-qemu-windows.ps1"
        Write-Info "WSL: sudo apt-get install qemu-system-x86"
        exit 1
    }
    Write-Success "QEMU trouvé: WSL ($wslQemu)"
    $useWSL = $true
} else {
    Write-Success "QEMU trouvé: $($qemu.Source)"
    $useWSL = $false
}

# Arguments QEMU
$qemuArgs = @(
    "-cdrom", $isoFullPath,
    "-boot", "d",
    "-m", "$($Memory)M",
    "-no-reboot",
    "-no-shutdown",
    "-serial", "stdio",
    "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"
)

Write-Info "Mémoire: $Memory MB"
Write-Info "Mode: GUI (fenêtre graphique)"
Write-Info "Serial: stdio (logs dans la console)"

Write-Host "==========================================" -ForegroundColor Blue
Write-Info "Lancement de QEMU..."
Write-Info "Pour quitter: fermez la fenêtre QEMU ou Ctrl+C dans la console"
Write-Host "==========================================" -ForegroundColor Blue

# Lancer QEMU
if ($useWSL) {
    # Convertir le chemin Windows en chemin WSL
    $wslIsoPath = $isoFullPath -replace '^([A-Z]):', '/mnt/$1' -replace '\\', '/' | ForEach-Object { $_.ToLower() }
    Write-Info "Lancement via WSL..."
    
    if ($Timeout -gt 0) {
        Write-Info "Timeout: ${Timeout}s"
        wsl timeout ${Timeout}s qemu-system-x86_64 -cdrom "$wslIsoPath" -boot d -m "$($Memory)M" -no-reboot -no-shutdown -serial stdio -device isa-debug-exit,iobase=0xf4,iosize=0x04
    } else {
        wsl qemu-system-x86_64 -cdrom "$wslIsoPath" -boot d -m "$($Memory)M" -no-reboot -no-shutdown -serial stdio -device isa-debug-exit,iobase=0xf4,iosize=0x04
    }
} else {
    if ($Timeout -gt 0) {
        Write-Info "Timeout: ${Timeout}s"
        $job = Start-Job -ScriptBlock {
            param($exe, $args)
            & $exe $args
        } -ArgumentList $qemu.Source, $qemuArgs
        
        Wait-Job $job -Timeout $Timeout | Out-Null
        if ($job.State -eq "Running") {
            Write-Info "Timeout atteint, arrêt de QEMU..."
            Stop-Job $job
            Remove-Job $job
        } else {
            Receive-Job $job
            Remove-Job $job
        }
    } else {
        & $qemu.Source $qemuArgs
    }
}

Write-Host "==========================================" -ForegroundColor Blue
Write-Success "Session QEMU terminée"
Write-Host "==========================================" -ForegroundColor Blue
