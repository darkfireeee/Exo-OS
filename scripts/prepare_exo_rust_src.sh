#!/usr/bin/env bash
set -euo pipefail

# Materialize an isolated Rust sysroot library tree for `-Z build-std` without
# mutating `$(rustc --print sysroot)`. Keeping `std` as a synced copy inside
# `$OUT_DIR/std` alongside symlinks to sibling crates (`core`, `alloc`,
# `portable-simd`, etc.) ensures `#[path = "../../portable-simd/..."]` in
# `library/std/src/lib.rs` resolves inside `$OUT_DIR`.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
OUT_DIR="${1:-${REPO_ROOT}/target/exo-rust-src/library}"

SYSROOT="$(rustc --print sysroot)"
UPSTREAM_LIB="${SYSROOT}/lib/rustlib/src/rust/library"

if [[ ! -d "${UPSTREAM_LIB}" ]]; then
    echo "error: upstream rust-src library directory not found: ${UPSTREAM_LIB}" >&2
    echo "hint: install component rust-src for active toolchain" >&2
    exit 1
fi

mkdir -p "${OUT_DIR}"

# Remove stale entries in OUT_DIR that are no longer in UPSTREAM_LIB (except std)
for existing in "${OUT_DIR}"/*; do
    [[ -e "${existing}" || -L "${existing}" ]] || continue
    base="$(basename "${existing}")"
    if [[ "${base}" == "std" ]]; then
        continue
    fi
    if [[ ! -e "${UPSTREAM_LIB}/${base}" ]]; then
        rm -rf "${existing}"
    fi
done

for entry in "${UPSTREAM_LIB}"/*; do
    base="$(basename "${entry}")"
    if [[ "${base}" == "std" || "${base}" == "std.orig" ]]; then
        continue
    fi
    target_path="${OUT_DIR}/${base}"
    if [[ -L "${target_path}" ]]; then
        current="$(readlink "${target_path}")"
        if [[ "${current}" == "${entry}" ]]; then
            continue
        fi
        rm -f "${target_path}"
    elif [[ -e "${target_path}" ]]; then
        rm -rf "${target_path}"
    fi
    ln -s "${entry}" "${target_path}"
done

if [[ -L "${OUT_DIR}/std" ]]; then
    rm -f "${OUT_DIR}/std"
fi
mkdir -p "${OUT_DIR}/std"
rsync -rt --no-perms --no-owner --no-group --modify-window=2 --delete "${REPO_ROOT}/library/std/" "${OUT_DIR}/std/"
