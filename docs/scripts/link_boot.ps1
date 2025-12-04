# link_boot.ps1 - Script PowerShell pour lier les fichiers objets boot
# Version Windows du script de linkage pour Exo-OS

param(
    [string]$OutDir = "target\x86_64-unknown-none\debug"
)

$ErrorActionPreference = "Stop"

Write-Host "=== Exo-OS Boot Linker Script (Windows) ===" -ForegroundColor Cyan
Write-Host "OUT_DIR: $OutDir"

$BootAsm = "kernel\src\arch\x86_64\boot\boot.asm"
$BootC = "kernel\src\arch\x86_64\boot\boot.c"
$BootObjDir = "$OutDir\boot_objs"

# Créer le répertoire de sortie
New-Item -ItemType Directory -Force -Path $BootObjDir | Out-Null

# Compiler boot.asm avec NASM
Write-Host "[1/4] Compiling boot.asm with NASM..." -ForegroundColor Yellow
$nasmArgs = @(
    "-f", "elf64",
    "-o", "$BootObjDir\boot.o",
    $BootAsm
)
& nasm $nasmArgs
if ($LASTEXITCODE -ne 0) {
    throw "NASM compilation failed!"
}

# Compiler boot.c avec Clang (génère LLVM bitcode compatible rust-lld)
Write-Host "[2/4] Compiling boot.c with Clang..." -ForegroundColor Yellow

# Essayer clang d'abord (préféré pour LLVM bitcode)
$useClang = $false
try {
    $clangVersion = clang --version 2>$null
    if ($LASTEXITCODE -eq 0) {
        $useClang = $true
        Write-Host "  Using Clang for LLVM bitcode" -ForegroundColor Gray
    }
} catch {}

if ($useClang) {
    # Clang: compile vers LLVM bitcode puis vers objet
    & clang -target x86_64-unknown-none `
        -c $BootC `
        -o "$BootObjDir\boot_c.o" `
        -ffreestanding `
        -nostdlib `
        -fno-builtin `
        -fno-stack-protector `
        -mno-red-zone `
        -fno-pic `
        -fno-pie `
        -m64
} else {
    Write-Host "  Clang not found, using GCC (may have linking issues)" -ForegroundColor Yellow
    # Fallback GCC
    & gcc -c $BootC `
        -o "$BootObjDir\boot_c.o" `
        -ffreestanding `
        -nostdlib `
        -fno-builtin `
        -fno-stack-protector `
        -mno-red-zone `
        -fno-pic `
        -fno-pie `
        -m64
}

if ($LASTEXITCODE -ne 0) {
    throw "C compilation failed!"
}

# Créer une archive statique avec ar
Write-Host "[3/4] Creating static library libboot_combined.a..." -ForegroundColor Yellow
$arArgs = @(
    "rcs",
    "$BootObjDir\libboot_combined.a",
    "$BootObjDir\boot.o",
    "$BootObjDir\boot_c.o"
)
& ar $arArgs
if ($LASTEXITCODE -ne 0) {
    throw "ar archiving failed!"
}

# Copier dans le répertoire cargo et tous les sous-répertoires build
Write-Host "[4/4] Copying to cargo output directory..." -ForegroundColor Yellow
Copy-Item "$BootObjDir\libboot_combined.a" -Destination "$OutDir\" -Force

# Copier aussi dans les répertoires OUT_DIR de cargo (pour build.rs)
$BuildDirs = Get-ChildItem -Path "$OutDir\build" -Filter "exo-kernel-*" -Directory -ErrorAction SilentlyContinue
foreach ($BuildDir in $BuildDirs) {
    $OutPath = Join-Path $BuildDir.FullName "out"
    if (Test-Path $OutPath) {
        Copy-Item "$BootObjDir\libboot_combined.a" -Destination "$OutPath\" -Force
        Write-Host "  Copied to: $OutPath" -ForegroundColor Gray
    }
}

Write-Host "✅ Boot objects linked successfully!" -ForegroundColor Green
Write-Host "Library: $OutDir\libboot_combined.a" -ForegroundColor Green
