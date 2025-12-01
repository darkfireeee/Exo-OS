#!/usr/bin/env pwsh
# Quick test script for Exo-OS
# Builds and tests the kernel with pretty output

param(
    [switch]$NoRebuild,
    [switch]$Verbose
)

$ErrorActionPreference = "Continue"

# Colors
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

function Write-Error-Custom {
    param([string]$Text)
    Write-Host "✗ $Text" -ForegroundColor Red
}

# Banner
Clear-Host
Write-Banner "EXO-OS QUICK TEST" "Cyan"

# Step 1: Build
if (-not $NoRebuild) {
    Write-Step "Building kernel..."
    $buildOutput = .\build.ps1 -Release 2>&1
    
    if ($LASTEXITCODE -eq 0) {
        Write-Success "Build completed successfully"
        
        # Extract file sizes
        $kernelSize = (Get-Item build\kernel.bin -ErrorAction SilentlyContinue).Length
        $isoSize = (Get-Item build\exo_os.iso -ErrorAction SilentlyContinue).Length
        
        if ($kernelSize) {
            Write-Host "  Kernel: $([math]::Round($kernelSize/1KB, 2)) KB" -ForegroundColor Gray
        }
        if ($isoSize) {
            Write-Host "  ISO: $([math]::Round($isoSize/1MB, 2)) MB" -ForegroundColor Gray
        }
    } else {
        Write-Error-Custom "Build failed!"
        if ($Verbose) {
            Write-Host ""
            Write-Host "Build output:" -ForegroundColor Yellow
            $buildOutput | Select-Object -Last 20
        }
        exit 1
    }
} else {
    Write-Step "Skipping rebuild (using existing binary)"
}

Write-Host ""

# Step 2: Check files
Write-Step "Checking files..."
$filesOk = $true

if (Test-Path "build\kernel.bin") {
    Write-Success "kernel.bin found"
} else {
    Write-Error-Custom "kernel.bin not found"
    $filesOk = $false
}

if (Test-Path "build\exo_os.iso") {
    Write-Success "exo_os.iso found"
} else {
    Write-Error-Custom "exo_os.iso not found"
    $filesOk = $false
}

if (-not $filesOk) {
    Write-Host ""
    Write-Error-Custom "Missing required files. Run without -NoRebuild flag."
    exit 1
}

Write-Host ""

# Step 3: Run QEMU
Write-Step "Launching QEMU..."
Write-Host "  (Close QEMU window or press Ctrl+C to stop)" -ForegroundColor Gray
Write-Host ""

.\scripts\test_qemu.ps1

# Wait for QEMU to finish
Start-Sleep -Seconds 2

Write-Host ""
Write-Step "Reading serial output..."

if (Test-Path "serial.log") {
    $serialContent = Get-Content serial.log -Raw
    
    # Check for success markers
    $hasRustEntry = $serialContent -match "RUST ENTRY POINT"
    $hasMagicValid = $serialContent -match "Valid Multiboot2 magic"
    $hasKernelReady = $serialContent -match "KERNEL READY"
    
    Write-Host ""
    Write-Banner "BOOT ANALYSIS" "Cyan"
    
    if ($hasRustEntry) {
        Write-Success "Rust kernel entry reached"
    } else {
        Write-Error-Custom "Rust kernel entry NOT reached"
    }
    
    if ($hasMagicValid) {
        Write-Success "Multiboot2 magic validated"
    } else {
        Write-Error-Custom "Multiboot2 magic validation failed"
    }
    
    if ($hasKernelReady) {
        Write-Success "Kernel ready state achieved"
    } else {
        Write-Error-Custom "Kernel did not reach ready state"
    }
    
    # Overall status
    Write-Host ""
    if ($hasRustEntry -and $hasMagicValid -and $hasKernelReady) {
        Write-Host "╔═══════════════════════════════════════════════════════╗" -ForegroundColor Green
        Write-Host "║                                                       ║" -ForegroundColor Green
        Write-Host "║       🎉 BOOT SUCCESSFUL - ALL CHECKS PASSED 🎉      ║" -ForegroundColor Green
        Write-Host "║                                                       ║" -ForegroundColor Green
        Write-Host "╚═══════════════════════════════════════════════════════╝" -ForegroundColor Green
    } else {
        Write-Host "╔═══════════════════════════════════════════════════════╗" -ForegroundColor Red
        Write-Host "║                                                       ║" -ForegroundColor Red
        Write-Host "║       ⚠️  BOOT INCOMPLETE - SOME CHECKS FAILED       ║" -ForegroundColor Red
        Write-Host "║                                                       ║" -ForegroundColor Red
        Write-Host "╚═══════════════════════════════════════════════════════╝" -ForegroundColor Red
    }
    
    Write-Host ""
    Write-Host "Full serial log:" -ForegroundColor Yellow
    Write-Host "─────────────────────────────────────────────────────────" -ForegroundColor Gray
    Write-Host $serialContent
    Write-Host "─────────────────────────────────────────────────────────" -ForegroundColor Gray
    
} else {
    Write-Error-Custom "serial.log not found"
}

Write-Host ""
Write-Host "Test completed at $(Get-Date -Format 'HH:mm:ss')" -ForegroundColor Gray
Write-Host ""
