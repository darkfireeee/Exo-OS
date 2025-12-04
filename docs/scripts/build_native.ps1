#!/usr/bin/env pwsh
# build_native.ps1 - Build Exo-OS nativement sous Windows
# Usage: .\scripts\build_native.ps1 [-Release] [-Clean] [-SkipBoot]

param(
    [switch]$Release,
    [switch]$Clean,
    [switch]$SkipBoot
)

$ErrorActionPreference = "Stop"

# Config
$Target = "x86_64-unknown-none"
$BuildMode = if ($Release) { "release" } else { "debug" }
$TargetDir = "target\$Target\$BuildMode"

function Write-Banner {
    param([string]$Text, [ConsoleColor]$Color = "Cyan")
    Write-Host ""
    Write-Host "═══════════════════════════════════════════════════════════" -ForegroundColor $Color
    Write-Host "  $Text" -ForegroundColor $Color
    Write-Host "═══════════════════════════════════════════════════════════" -ForegroundColor $Color
    Write-Host ""
}

function Write-Step {
    param([string]$Text)
    Write-Host "→ $Text" -ForegroundColor Yellow
}

function Write-Success {
    param([string]$Text)
    Write-Host "✓ $Text" -ForegroundColor Green
}

function Write-Fail {
    param([string]$Text)
    Write-Host "✗ $Text" -ForegroundColor Red
}

# Banner
Write-Banner "EXO-OS BUILD (Windows Native)"

Write-Host "Build mode: $BuildMode" -ForegroundColor Gray
Write-Host "Target: $Target" -ForegroundColor Gray
Write-Host ""

# Check prerequisites
Write-Step "Vérification des prérequis..."

$missingTools = @()

# Check Rust
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $missingTools += "cargo (Rust)"
}

# Check NASM (for boot.asm)
if (-not (Get-Command nasm -ErrorAction SilentlyContinue)) {
    $missingTools += "nasm"
}

# Check Clang or GCC (for boot.c)
$hasClang = Get-Command clang -ErrorAction SilentlyContinue
$hasGcc = Get-Command gcc -ErrorAction SilentlyContinue
if (-not $hasClang -and -not $hasGcc) {
    $missingTools += "clang ou gcc"
}

# Check ar (for libboot_combined.a)
if (-not (Get-Command ar -ErrorAction SilentlyContinue)) {
    $missingTools += "ar (llvm-ar ou binutils)"
}

if ($missingTools.Count -gt 0) {
    Write-Fail "Outils manquants:"
    foreach ($tool in $missingTools) {
        Write-Host "  - $tool" -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "Installez les outils manquants:" -ForegroundColor Yellow
    Write-Host "  - Rust: https://rustup.rs/" -ForegroundColor White
    Write-Host "  - NASM: choco install nasm" -ForegroundColor White
    Write-Host "  - LLVM: choco install llvm" -ForegroundColor White
    exit 1
}
Write-Success "Tous les prérequis sont installés"
Write-Host ""

# Clean if requested
if ($Clean) {
    Write-Step "Nettoyage..."
    cargo clean --manifest-path kernel/Cargo.toml
    Remove-Item -Path "build" -Recurse -Force -ErrorAction SilentlyContinue
    Write-Success "Nettoyage terminé"
    Write-Host ""
}

# Check Rust target installed
Write-Step "Vérification de la target Rust..."
$targets = rustup target list --installed
if ($targets -notcontains $Target) {
    Write-Host "  Installation de la target $Target..." -ForegroundColor Gray
    rustup target add $Target
}
Write-Success "Target $Target installée"
Write-Host ""

# Compile boot objects
if (-not $SkipBoot) {
    Write-Step "Compilation des fichiers boot (ASM + C)..."
    
    $BootObjDir = "$TargetDir\boot_objs"
    New-Item -ItemType Directory -Force -Path $BootObjDir | Out-Null
    
    # Compile boot.asm
    Write-Host "  [1/3] NASM: boot.asm -> boot.o" -ForegroundColor Gray
    & nasm -f elf64 -o "$BootObjDir\boot.o" "kernel\src\arch\x86_64\boot\boot.asm"
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "NASM compilation failed!"
        exit 1
    }
    
    # Compile boot.c
    Write-Host "  [2/3] Clang/GCC: boot.c -> boot_c.o" -ForegroundColor Gray
    $cc = if ($hasClang) { "clang" } else { "gcc" }
    
    & $cc -target x86_64-unknown-none `
        -c "kernel\src\arch\x86_64\boot\boot.c" `
        -o "$BootObjDir\boot_c.o" `
        -ffreestanding `
        -nostdlib `
        -fno-builtin `
        -fno-stack-protector `
        -mno-red-zone `
        -fno-pic `
        -fno-pie `
        -m64
    
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "C compilation failed!"
        exit 1
    }
    
    # Create static library
    Write-Host "  [3/3] ar: Création de libboot_combined.a" -ForegroundColor Gray
    & ar rcs "$BootObjDir\libboot_combined.a" "$BootObjDir\boot.o" "$BootObjDir\boot_c.o"
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "Archive creation failed!"
        exit 1
    }
    
    # Copy to target directories
    Copy-Item "$BootObjDir\libboot_combined.a" -Destination "$TargetDir\" -Force
    
    Write-Success "Boot objects compilés"
    Write-Host ""
}

# Build kernel
Write-Step "Compilation du kernel Rust..."

$cargoArgs = @(
    "build",
    "--target", $Target,
    "--manifest-path", "kernel/Cargo.toml"
)

if ($Release) {
    $cargoArgs += "--release"
}

& cargo @cargoArgs

if ($LASTEXITCODE -ne 0) {
    Write-Fail "Cargo build failed!"
    exit 1
}

Write-Success "Kernel compilé"
Write-Host ""

# Copy output
Write-Step "Copie des fichiers de sortie..."

New-Item -ItemType Directory -Force -Path "build" | Out-Null
$kernelPath = "$TargetDir\exo-kernel"

if (Test-Path $kernelPath) {
    Copy-Item $kernelPath -Destination "build\kernel.bin" -Force
    $size = (Get-Item "build\kernel.bin").Length
    Write-Success "kernel.bin créé ($([math]::Round($size/1KB, 2)) KB)"
} else {
    Write-Fail "Kernel binary non trouvé!"
    exit 1
}

Write-Host ""
Write-Banner "BUILD TERMINÉ" "Green"

Write-Host "Fichiers générés:" -ForegroundColor Cyan
Write-Host "  build\kernel.bin - Kernel ELF" -ForegroundColor White
Write-Host ""
Write-Host "Pour créer une ISO bootable et tester:" -ForegroundColor Cyan
Write-Host "  .\scripts\test_qemu.ps1" -ForegroundColor White
Write-Host ""
