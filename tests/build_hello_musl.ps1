#!/usr/bin/env pwsh
# Build script for musl Hello World test

$ErrorActionPreference = "Stop"

Write-Host "Building musl Hello World test..." -ForegroundColor Cyan

# Paths
$MUSL_DIR = "kernel/src/posix_x/musl"
$TEST_SRC = "tests/integration/hello_musl.c"
$OUTPUT = "tests/integration/hello_musl.elf"

# Step 1: Compile musl library (if not already done)
Write-Host "`n[1/3] Checking musl library..." -ForegroundColor Yellow

if (!(Test-Path "$MUSL_DIR/lib/libc.a")) {
    Write-Host "Building musl library..." -ForegroundColor Yellow
    Push-Location $MUSL_DIR
    try {
        make -f Makefile.exo
        if ($LASTEXITCODE -ne 0) {
            throw "musl build failed"
        }
    } finally {
        Pop-Location
    }
    Write-Host "musl library built successfully!" -ForegroundColor Green
} else {
    Write-Host "musl library already exists" -ForegroundColor Green
}

# Step 2: Compile test program
Write-Host "`n[2/3] Compiling hello_musl.c..." -ForegroundColor Yellow

$CFLAGS = @(
    "-nostdlib",
    "-static",
    "-fno-stack-protector",
    "-I$MUSL_DIR/include",
    "-I$MUSL_DIR/arch/x86_64",
    "-DEXO_OS"
)

clang $CFLAGS -c $TEST_SRC -o tests/integration/hello_musl.o

if ($LASTEXITCODE -ne 0) {
    throw "Compilation failed"
}

Write-Host "Compiled successfully!" -ForegroundColor Green

# Step 3: Link with musl
Write-Host "`n[3/3] Linking with musl..." -ForegroundColor Yellow

clang -nostdlib -static `
    tests/integration/hello_musl.o `
    $MUSL_DIR/crt/crt1.o `
    $MUSL_DIR/crt/crti.o `
    $MUSL_DIR/lib/libc.a `
    $MUSL_DIR/crt/crtn.o `
    -o $OUTPUT

if ($LASTEXITCODE -ne 0) {
    throw "Linking failed"
}

Write-Host "`nBuild complete!" -ForegroundColor Green
Write-Host "Output: $OUTPUT" -ForegroundColor Cyan
Write-Host "`nTo run in Exo-OS, load this ELF as userspace process." -ForegroundColor Yellow
