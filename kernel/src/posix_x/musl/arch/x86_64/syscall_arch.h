/* syscall_arch.h - x86_64 syscall interface for Exo-OS
 *
 * MODIFIED FOR EXO-OS: Instead of using native syscall instruction,
 * we redirect all syscalls to our C bridge which calls Rust kernel.
 */

#define __SYSCALL_LL_E(x) (x)
#define __SYSCALL_LL_O(x) (x)

/* External C bridge functions (defined in exo_syscall_bridge.c) */
extern long __syscall0(long);
extern long __syscall1(long, long);
extern long __syscall2(long, long, long);
extern long __syscall3(long, long, long, long);
extern long __syscall4(long, long, long, long, long);
extern long __syscall5(long, long, long, long, long, long);
extern long __syscall6(long, long, long, long, long, long, long);

/* Cancellation point variant */
extern long __syscall_cp(long, long, long, long, long, long, long);

/* No error transformation needed - kernel returns errno directly */
#define __SYSCALL_NOERRNO

/* Inline wrappers that call our bridge */
static __inline long __syscall0_impl(long n) {
    return __syscall0(n);
}

static __inline long __syscall1_impl(long n, long a1) {
    return __syscall1(n, a1);
}

static __inline long __syscall2_impl(long n, long a1, long a2) {
    return __syscall2(n, a1, a2);
}

static __inline long __syscall3_impl(long n, long a1, long a2, long a3) {
    return __syscall3(n, a1, a2, a3);
}

static __inline long __syscall4_impl(long n, long a1, long a2, long a3, long a4) {
    return __syscall4(n, a1, a2, a3, a4);
}

static __inline long __syscall5_impl(long n, long a1, long a2, long a3, long a4, long a5) {
    return __syscall5(n, a1, a2, a3, a4, a5);
}

static __inline long __syscall6_impl(long n, long a1, long a2, long a3, long a4, long a5, long a6) {
    return __syscall6(n, a1, a2, a3, a4, a5, a6);
}

/* Cancellation point wrapper */
static __inline long __syscall_cp_impl(long n, long a1, long a2, long a3, long a4, long a5, long a6) {
    return __syscall_cp(n, a1, a2, a3, a4, a5, a6);
}

/* Map generic names to our implementations */
#define __syscall0 __syscall0_impl
#define __syscall1 __syscall1_impl
#define __syscall2 __syscall2_impl
#define __syscall3 __syscall3_impl
#define __syscall4 __syscall4_impl
#define __syscall5 __syscall5_impl
#define __syscall6 __syscall6_impl
#define __syscall_cp __syscall_cp_impl
