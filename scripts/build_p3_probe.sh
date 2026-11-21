#!/usr/bin/env bash
set -euo pipefail

# Builds the Phase 3 verification probes into target/p3-probe/:
#   p3dyn   PIE, PT_INTERP=/lib/ld-exo.so, DT_NEEDED libp3a.so -> libp3b.so, dlopen libp3c.so
#   p3musl  static-PIE against musl-exo (scripts/build_musl_exo.sh must have run)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SRC="${REPO_ROOT}/userspace/p3-probe"
OUT="${REPO_ROOT}/target/p3-probe"
SYSROOT="${REPO_ROOT}/target/musl-exo/sysroot"
mkdir -p "${OUT}"
cd "${OUT}"

CFLAGS_FREE=(-target x86_64-unknown-none-elf -ffreestanding -fno-stack-protector -fno-builtin -O2 -fno-asynchronous-unwind-tables -mno-red-zone)
LDCOMMON=(-z noexecstack -z relro -z now -z separate-code)

clang "${CFLAGS_FREE[@]}" -fPIC -c "${SRC}/libp3b.c" -o libp3b.o
ld.lld -shared "${LDCOMMON[@]}" -soname libp3b.so -o libp3b.so libp3b.o

clang "${CFLAGS_FREE[@]}" -fPIC -c "${SRC}/libp3a.c" -o libp3a.o
ld.lld -shared "${LDCOMMON[@]}" -soname libp3a.so -o libp3a.so libp3a.o -L. -lp3b

clang "${CFLAGS_FREE[@]}" -fPIC -c "${SRC}/libp3c.c" -o libp3c.o
ld.lld -shared "${LDCOMMON[@]}" -soname libp3c.so -o libp3c.so libp3c.o

clang "${CFLAGS_FREE[@]}" -fPIE -c "${SRC}/p3dyn.c" -o p3dyn.o
ld.lld -pie "${LDCOMMON[@]}" -z undefs --dynamic-linker=/lib/ld-exo.so --no-as-needed \
    -o p3dyn p3dyn.o -L. -lp3a

if [[ -f "${SYSROOT}/lib/libc.a" ]]; then
    gcc -std=gnu11 -O2 -static-pie -fPIE -nostdinc -nostdlib -fno-stack-protector \
        -isystem "${SYSROOT}/include" "${SRC}/p3musl.c" \
        "${SYSROOT}/lib/rcrt1.o" "${SYSROOT}/lib/crti.o" \
        "${SYSROOT}/lib/libc.a" -lgcc "${SYSROOT}/lib/crtn.o" \
        -Wl,-z,noexecstack -Wl,-z,separate-code -o p3musl
else
    echo "[p3-probe] SKIP p3musl (run scripts/build_musl_exo.sh first)" >&2
fi
for f in p3dyn libp3a.so libp3b.so libp3c.so p3musl; do
    [[ -f "$f" ]] && llvm-strip --strip-sections "$f" 2>/dev/null || true
done
ls -l "${OUT}"
echo "[p3-probe] OK"
