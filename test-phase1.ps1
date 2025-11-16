# Test Phase 1 - Lancer le kernel optimisé avec mesures de performance
# Utilise l'ISO bootable avec QEMU

Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Phase 1: Test Kernel Optimisé" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host ""

# Arrêter toute instance QEMU
Stop-Process -Name "qemu-system-x86_64" -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

# Chemins
$ISO = "C:\Users\Eric\Documents\Exo-OS\build\exo-os.iso"
$LOG = "C:\Users\Eric\Documents\Exo-OS\boot-test.log"
$QEMU = "C:\Program Files\qemu\qemu-system-x86_64.exe"

# Vérifications
if (-not (Test-Path $ISO)) {
    Write-Host "✗ ISO non trouvée: $ISO" -ForegroundColor Red
    exit 1
}

if (-not (Test-Path $QEMU)) {
    Write-Host "✗ QEMU non trouvé: $QEMU" -ForegroundColor Red
    exit 1
}

# Supprimer ancien log
if (Test-Path $LOG) {
    Remove-Item $LOG -Force
}

Write-Host "[1/3] Lancement QEMU..." -ForegroundColor Yellow
Write-Host "  → ISO: $([math]::Round((Get-Item $ISO).Length/1MB, 2)) MB" -ForegroundColor Gray
try { Write-Host "  → SHA-256: $((Get-FileHash $ISO -Algorithm SHA256).Hash)" -ForegroundColor Gray } catch {}
Write-Host "  → Mémoire: 512 MB" -ForegroundColor Gray
Write-Host "  → Log: boot-test.log" -ForegroundColor Gray
Write-Host ""

# Timestamp début
$startTime = Get-Date

# Lancer QEMU en background avec timeout
$job = Start-Job -ScriptBlock {
    param($qemu, $iso, $log)
    & $qemu -cdrom $iso -serial "file:$log" -display none -m 512M -no-reboot -no-shutdown
} -ArgumentList $QEMU, $ISO, $LOG

Write-Host "[2/3] Attente du boot (timeout 10s)..." -ForegroundColor Yellow

# Attendre que le log apparaisse ou timeout
$timeout = 10
$elapsed = 0
while ($elapsed -lt $timeout) {
    Start-Sleep -Milliseconds 500
    $elapsed += 0.5
    
    if (Test-Path $LOG) {
        $content = Get-Content $LOG -Raw -ErrorAction SilentlyContinue
        if ($content -match "initialisé avec succès|Kernel panic|Error") {
            break
        }
    }
    
    # Afficher progression
    $progress = [math]::Round(($elapsed / $timeout) * 100)
    Write-Host "`r  → $progress% ($elapsed s)" -NoNewline -ForegroundColor Gray
}

Write-Host ""
$bootTime = ((Get-Date) - $startTime).TotalMilliseconds

Write-Host ""
Write-Host "[3/3] Résultats..." -ForegroundColor Yellow

# Arrêter QEMU
Stop-Job $job -ErrorAction SilentlyContinue
Remove-Job $job -Force -ErrorAction SilentlyContinue
Stop-Process -Name "qemu-system-x86_64" -Force -ErrorAction SilentlyContinue

# Analyser le log
if (Test-Path $LOG) {
    $logContent = Get-Content $LOG -Raw
    
    Write-Host ""
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    Write-Host " Sortie Kernel:" -ForegroundColor Cyan
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    Get-Content $LOG | ForEach-Object {
        if ($_ -match "Error|panic|FAIL") {
            Write-Host $_ -ForegroundColor Red
        } elseif ($_ -match "succès|OK|SUCCESS") {
            Write-Host $_ -ForegroundColor Green
        } else {
            Write-Host $_
        }
    }
    
    Write-Host ""
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    Write-Host " Métriques Phase 1:" -ForegroundColor Cyan
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    
    # Temps de boot (approximatif - du lancement QEMU au message de succès)
    Write-Host "  Boot time: ~$([math]::Round($bootTime)) ms" -ForegroundColor $(if($bootTime -lt 800) {"Green"} else {"Yellow"})
    Write-Host "    Objectif: < 800 ms" -ForegroundColor Gray
    
    # Taille binaire
    $kernelSize = (Get-Item "target\x86_64-unknown-none\release\exo-kernel").Length / 1KB
    Write-Host "  Binary size: $([math]::Round($kernelSize, 2)) KB" -ForegroundColor Green
    Write-Host "    Objectif: < 3 MB (3072 KB)" -ForegroundColor Gray
    
    # Mémoire (TODO: parser du log si disponible)
    if ($logContent -match "RAM détectée: (\d+) MB") {
        $ramDetected = $Matches[1]
        Write-Host "  RAM détectée: $ramDetected MB" -ForegroundColor Cyan
    }
    if ($logContent -match "Tas de (\d+) MB") {
        $heapSize = $Matches[1]
        Write-Host "  Heap utilisé: $heapSize MB" -ForegroundColor $(if($heapSize -lt 64) {"Green"} else {"Yellow"})
        Write-Host "    Objectif: < 64 MB" -ForegroundColor Gray
    }
    
    Write-Host ""
    
    # Statut global
    if ($logContent -match "initialisé avec succès") {
        Write-Host "✓ Kernel boot: SUCCÈS" -ForegroundColor Green
    } elseif ($logContent -match "panic") {
        Write-Host "✗ Kernel boot: PANIC" -ForegroundColor Red
    } else {
        Write-Host "⚠ Kernel boot: INCERTAIN (timeout?)" -ForegroundColor Yellow
    }
    
    Write-Host ""
    
} else {
    Write-Host "✗ Aucune sortie serial détectée" -ForegroundColor Red
    Write-Host "  Le kernel n'a peut-être pas produit de sortie" -ForegroundColor Gray
}

Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
