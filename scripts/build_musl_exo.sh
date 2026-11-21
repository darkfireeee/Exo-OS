#!/usr/bin/env bash
set -euo pipefail

# Out-of-tree build of libs/musl-exo (static libc.a + crt1.o/Scrt1.o/rcrt1.o) into
# target/musl-exo/{build,sysroot}. Does not touch the source tree.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SRC="${REPO_ROOT}/libs/musl-exo"
OUT="${REPO_ROOT}/target/musl-exo"
BUILD="${OUT}/build"
SYSROOT="${OUT}/sysroot"

# musl's configure/Makefile are POSIX-sh/make based and fail on DrvFs checkouts when
# the tree has CRLF; stage a clean LF copy under target/ if needed.
mkdir -p "${BUILD}" "${SYSROOT}"
if [[ "${FORCE_MUSL_REBUILD:-0}" != "1" && -f "${SYSROOT}/lib/libc.a" && -f "${SYSROOT}/lib/rcrt1.o" ]]; then
    if [[ -z "$(find "${SRC}" -type f -newer "${SYSROOT}/lib/libc.a" -print -quit 2>/dev/null)" ]]; then
        echo "[musl-exo] Up-to-date: ${SYSROOT}/lib/libc.a"
        exit 0
    fi
fi
STAGE="${OUT}/src"
rsync -rt --delete --no-perms --no-owner --no-group "${SRC}/" "${STAGE}/"
find "${STAGE}" -type f \( -name '*.sh' -o -name 'configure' -o -name '*.mak' -o -name 'Makefile' -o -name '*.awk' -o -name '*.in' \) -print0 | xargs -0 sed -i 's/\r$//'
chmod +x "${STAGE}/configure"

cd "${BUILD}"
# NB: ne pas passer --target=x86_64 : configure en déduirait CROSS_COMPILE=x86_64- (x86_64-ar introuvable).
CONF_STAMP="gcc=${MUSL_EXO_CC:-gcc};ar=${MUSL_EXO_AR:-ar}"
if [[ ! -f config.mak || ! -f .conf-stamp || "$(cat .conf-stamp)" != "${CONF_STAMP}" || "${STAGE}/configure" -nt config.mak ]]; then
    CC="${MUSL_EXO_CC:-gcc}" AR="${MUSL_EXO_AR:-ar}" RANLIB="${MUSL_EXO_RANLIB:-ranlib}" "${STAGE}/configure" \
        --prefix="${SYSROOT}" --disable-shared --enable-static --disable-wrapper
    printf '%s' "${CONF_STAMP}" > .conf-stamp
fi
make -j"$(nproc)" lib/libc.a lib/crt1.o lib/Scrt1.o lib/rcrt1.o lib/crti.o lib/crtn.o
make install-headers >/dev/null
install -D -m 644 lib/libc.a "${SYSROOT}/lib/libc.a"
for o in crt1 Scrt1 rcrt1 crti crtn; do install -D -m 644 "lib/${o}.o" "${SYSROOT}/lib/${o}.o"; done
echo "[musl-exo] OK: ${SYSROOT}/lib/libc.a"
