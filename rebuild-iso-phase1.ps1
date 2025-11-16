# Script Phase 1: Rebuild ISO avec kernel optimisé
# Ce script reconstruit l'ISO avec le kernel optimisé en release

Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Phase 1: Rebuild ISO avec Optimisations" -ForegroundColor Cyan  
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host ""

# Chemins
$kernelBin = "target\x86_64-unknown-none\release\exo-kernel"
$isoDest = "isodir\boot\kernel.bin"
$isoFile = "build\exo-os.iso"

# 1. Recompiler le kernel (release + features recommandées)
$features = "fusion_rings,hybrid_allocator"
if ($env:EXO_QEMU_EXIT -ne "0") { $features += ",bench_auto_exit" }
Write-Host "[1/3] Compilation du kernel (release + features $features)..." -ForegroundColor Yellow

# Construire via WSL pour garantir l'environnement toolchain
$wslExists = Get-Command wsl -ErrorAction SilentlyContinue
if ($wslExists) {
    $wslPath = "/mnt/c/Users/Eric/Documents/Exo-OS"
    $buildOut = wsl -- bash -lc "cd '$wslPath' && cargo build --release -p exo-kernel --features $features 2>&1"
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  ✗ Échec compilation kernel (WSL)" -ForegroundColor Red
        Write-Host $buildOut -ForegroundColor DarkGray
        exit 1
    } else {
        Write-Host "  ✓ Compilation OK" -ForegroundColor Green
    }
} else {
    Write-Host "  ⚠ WSL non disponible, tentative compilation native..." -ForegroundColor Yellow
    $buildOut = cargo build --release -p exo-kernel --features $features 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  ✗ Échec compilation kernel (native)" -ForegroundColor Red
        Write-Host $buildOut -ForegroundColor DarkGray
        exit 1
    } else {
        Write-Host "  ✓ Compilation OK (native)" -ForegroundColor Green
    }
}

# 2. Copier le kernel optimisé
Write-Host "[2/4] Copie du kernel optimisé..." -ForegroundColor Yellow
if (Test-Path $kernelBin) {
    Copy-Item $kernelBin $isoDest -Force
    $size = (Get-Item $isoDest).Length
    Write-Host "  ✓ Kernel copié: $([math]::Round($size/1KB, 2)) KB" -ForegroundColor Green
} else {
    Write-Host "  ✗ Kernel non trouvé: $kernelBin" -ForegroundColor Red
    exit 1
}

# 3. Créer l'ISO avec grub-mkrescue (via WSL si disponible, sinon message)
Write-Host "[3/4] Création de l'ISO..." -ForegroundColor Yellow

# Tenter via WSL
$wslExists = Get-Command wsl -ErrorAction SilentlyContinue
if ($wslExists) {
    $wslPath = "/mnt/c/Users/Eric/Documents/Exo-OS"
    Write-Host "  → Utilisation de grub-mkrescue via WSL..." -ForegroundColor Gray
    # Assurer le dossier build
    if (-not (Test-Path "build")) { New-Item -ItemType Directory -Path "build" | Out-Null }
    
    $result = wsl -- bash -c "cd '$wslPath' && mkdir -p build && grub-mkrescue -o build/exo-os.iso isodir 2>&1"
    
    if ($LASTEXITCODE -eq 0) {
        Write-Host "  ✓ ISO créée avec succès" -ForegroundColor Green
    } else {
        Write-Host "  ✗ Erreur grub-mkrescue" -ForegroundColor Red
        Write-Host "  $result" -ForegroundColor DarkGray
        exit 1
    }
} else {
    Write-Host "  ⚠ WSL non disponible" -ForegroundColor Yellow
    Write-Host "  → ISO peut être créée manuellement ou avec script bash" -ForegroundColor Gray
}

# 4. Afficher la taille de l'ISO
Write-Host "[4/4] Résultat..." -ForegroundColor Yellow
if (Test-Path $isoFile) {
    $isoSize = (Get-Item $isoFile).Length
    Write-Host "  ✓ ISO: $([math]::Round($isoSize/1MB, 2)) MB" -ForegroundColor Green
    try {
        $hash = (Get-FileHash $isoFile -Algorithm SHA256).Hash
        Write-Host "  ✓ SHA-256: $hash" -ForegroundColor Gray
    } catch {}
    Write-Host ""
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    Write-Host " Optimisations Phase 1 appliquées:" -ForegroundColor Green
    Write-Host "   • opt-level = z (taille minimale)" -ForegroundColor Gray
    Write-Host "   • LTO = fat (optimisation cross-crate)" -ForegroundColor Gray
    Write-Host "   • Sections supprimées (--gc-sections)" -ForegroundColor Gray
    Write-Host "   • Symbols stripped" -ForegroundColor Gray
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Pour tester: .\test-kernel.ps1" -ForegroundColor Yellow
} else {
    Write-Host "  ✗ ISO non trouvée" -ForegroundColor Red
    exit 1
}
