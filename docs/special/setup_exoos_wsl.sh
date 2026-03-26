#!/usr/bin/env bash
set -euo pipefail

# ExoOS WSL setup script
# Usage: bash docs/special/setup_exoos_wsl.sh

if [[ "${EUID}" -eq 0 ]]; then
  echo "[ERROR] Run this script as a normal user (it will use sudo internally)."
  exit 1
fi

echo "[1/5] Updating apt indexes..."
sudo apt-get update

echo "[2/5] Installing base build + virtualization dependencies..."
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
  build-essential \
  curl \
  git \
  pkg-config \
  clang \
  lld \
  llvm \
  llvm-dev \
  nasm \
  make \
  qemu-system-x86 \
  qemu-utils \
  grub-pc-bin \
  xorriso \
  mtools

echo "[3/5] Installing rustup if missing..."
if ! command -v rustup >/dev/null 2>&1; then
  curl https://sh.rustup.rs -sSf | sh -s -- -y
fi

# shellcheck disable=SC1090
source "${HOME}/.cargo/env"

echo "[4/5] Configuring Rust toolchain for ExoOS..."
rustup toolchain install nightly
rustup default nightly
rustup component add rust-src llvm-tools-preview
rustup target add x86_64-unknown-none

echo "[5/5] Environment validation..."
rustc --version
cargo --version
qemu-system-x86_64 --version | head -n 1

echo "\n[OK] WSL environment ready for ExoOS Phase 0 checks."
echo "Next suggested checks:"
echo "  - cd /mnt/c/Users/xavie/Desktop/Exo-OS/libs && cargo check"
echo "  - cd /mnt/c/Users/xavie/Desktop/Exo-OS/kernel && cargo check"
