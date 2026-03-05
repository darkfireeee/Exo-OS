#define __SYSCALL_LL_E(x) (x)
#define __SYSCALL_LL_O(x) (x)

static __inline long __syscall0(long n)
{
	unsigned long ret;
	__asm__ __volatile__ ("syscall" : "=a"(ret) : "a"(n) : "rcx", "r11", "memory");
	return ret;
}

static __inline long __syscall1(long n, long a1)
{
	unsigned long ret;
	__asm__ __volatile__ ("syscall" : "=a"(ret) : "a"(n), "D"(a1) : "rcx", "r11", "memory");
	return ret;
}

static __inline long __syscall2(long n, long a1, long a2)
{
	unsigned long ret;
	__asm__ __volatile__ ("syscall" : "=a"(ret) : "a"(n), "D"(a1), "S"(a2)
						  : "rcx", "r11", "memory");
	return ret;
}

static __inline long __syscall3(long n, long a1, long a2, long a3)
{
	unsigned long ret;
	__asm__ __volatile__ ("syscall" : "=a"(ret) : "a"(n), "D"(a1), "S"(a2),
						  "d"(a3) : "rcx", "r11", "memory");
	return ret;
}

static __inline long __syscall4(long n, long a1, long a2, long a3, long a4)
{
	unsigned long ret;
	register long r10 __asm__("r10") = a4;
	__asm__ __volatile__ ("syscall" : "=a"(ret) : "a"(n), "D"(a1), "S"(a2),
						  "d"(a3), "r"(r10): "rcx", "r11", "memory");
	return ret;
}

static __inline long __syscall5(long n, long a1, long a2, long a3, long a4, long a5)
{
	unsigned long ret;
	register long r10 __asm__("r10") = a4;
	register long r8 __asm__("r8") = a5;
	__asm__ __volatile__ ("syscall" : "=a"(ret) : "a"(n), "D"(a1), "S"(a2),
						  "d"(a3), "r"(r10), "r"(r8) : "rcx", "r11", "memory");
	return ret;
}

static __inline long __syscall6(long n, long a1, long a2, long a3, long a4, long a5, long a6)
{
	unsigned long ret;
	register long r10 __asm__("r10") = a4;
	register long r8 __asm__("r8") = a5;
	register long r9 __asm__("r9") = a6;
	__asm__ __volatile__ ("syscall" : "=a"(ret) : "a"(n), "D"(a1), "S"(a2),
						  "d"(a3), "r"(r10), "r"(r8), "r"(r9) : "rcx", "r11", "memory");
	return ret;
}

#define VDSO_USEFUL
/* Exo-OS : symboles VDSO exportés par le kernel Exo-OS */
#define VDSO_CGT_SYM "__exo_vdso_clock_gettime"
#define VDSO_CGT_VER "EXO_OS_1.0"
#define VDSO_GETCPU_SYM "__exo_vdso_getcpu"
#define VDSO_GETCPU_VER "EXO_OS_1.0"

#define IPC_64 0

/* ── Exo-OS : overrides des numéros syscall (LIB-01) ────────────────────────
 * Ces #undef/#define sont appliqués ICI, après que bits/syscall.h a défini
 * les numéros Linux standard. Seuls les syscalls incompatibles avec ExoFS
 * sont redirigés vers le handler Ring0 combine.
 *
 * BUG-01 fix : open()/openat() → SYS_EXOFS_OPEN_BY_PATH (519)
 *   musl envoie : syscall(519, path_ptr, flags, mode)
 *   kernel Ring0 : path_resolve() + object_open() atomiques
 *
 * BUG-02 fix : getdents/getdents64 → SYS_EXOFS_READDIR (520)
 *   Retourne : struct linux_dirent64 (compatible musl x86_64 64-bit)
 * ─────────────────────────────────────────────────────────────── */

/* open() : redirigé vers le syscall combiné ExoFS Ring0 */
#undef  __NR_open
#define __NR_open       519

/* openat() : même handler — AT_FDCWD ignoré (ExoFS résout depuis la racine) */
#undef  __NR_openat
#define __NR_openat     519

/* creat() alias vers open() avec O_CREAT|O_WRONLY|O_TRUNC */
#undef  __NR_creat
#define __NR_creat      519

/* getdents / getdents64 : redirigés vers SYS_EXOFS_READDIR */
#undef  __NR_getdents
#define __NR_getdents   520

#undef  __NR_getdents64
#define __NR_getdents64 520
