/* Phase 3 probe executable: PIE with PT_INTERP=/lib/ld-exo.so, freestanding.
 * Verifies ET_DYN kernel loading + ASLR, SysV entry stack, auxv, ld-exo.so
 * (DT_NEEDED BFS, relocations, TLS/DTV, dlopen/dlsym/dlclose/dlerror/dladdr/
 * dl_iterate_phdr) and MAP_FIXED / MAP_FIXED_NOREPLACE semantics. */
typedef unsigned long u64;
typedef long s64;

extern int p3a_add(int, int);
extern int p3a_mulcall(int, int);
extern int p3a_ctor_flag(void);
extern int p3a_tls_get(void);
extern int p3a_tbss_get(void);
extern void p3a_tls_set(int);

void *dlopen(const char *, int);
void *dlsym(void *, const char *);
int dlclose(void *);
char *dlerror(void);
struct dl_info { const char *fname; void *fbase; const char *sname; void *saddr; };
int dladdr(const void *, struct dl_info *);
struct dl_phdr_info_min { u64 addr; const char *name; const void *phdr; unsigned short phnum; };
int dl_iterate_phdr(int (*)(struct dl_phdr_info_min *, u64, void *), void *);

static __thread int exe_tls = 5;
static __thread int exe_tbss;

static s64 sc(s64 n, s64 a, s64 b, s64 c, s64 d, s64 e, s64 f)
{
    s64 ret;
    register s64 r10 __asm__("r10") = d;
    register s64 r8 __asm__("r8") = e;
    register s64 r9 __asm__("r9") = f;
    __asm__ volatile("syscall" : "=a"(ret)
                     : "0"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9)
                     : "rcx", "r11", "memory");
    return ret;
}

static u64 slen(const char *s) { u64 n = 0; while (s[n]) n++; return n; }
static void out(const char *s) { sc(1, 1, (s64)s, (s64)slen(s), 0, 0, 0); }
static int seq(const char *a, const char *b)
{
    while (*a && *a == *b) { a++; b++; }
    return *a == *b;
}
static int contains(const char *h, const char *needle)
{
    u64 n = slen(needle);
    for (; *h; h++) {
        u64 i = 0;
        while (i < n && h[i] == needle[i]) i++;
        if (i == n) return 1;
    }
    return 0;
}

static int failures;
static void check(int ok, const char *name)
{
    out(ok ? "[P3_PROBE] PASS " : "[P3_PROBE] FAIL ");
    out(name);
    out("\n");
    if (!ok) failures++;
}

static int phdr_count;
static int phdr_cb(struct dl_phdr_info_min *info, u64 size, void *data)
{
    (void)size; (void)data; (void)info;
    phdr_count++;
    return 0;
}

#define PROT_RW 3
#define MAP_PRIVATE_ANON 0x22
#define MAP_FIXED 0x10
#define MAP_FIXED_NOREPLACE 0x100000

void p3_main(u64 *sp, u64 align_probe)
{
    out("[P3_PROBE] start\n");
    check(align_probe == 0, "sysv_entry_rsp_16_aligned");

    u64 argc = sp[0];
    char **argv = (char **)(sp + 1);
    u64 *auxv = (u64 *)(argv + argc + 1);
    while (*auxv) auxv++;           /* skip envp[] */
    auxv++;
    u64 at_base = 0, at_phdr = 0, at_random = 0, at_secure = 99, at_entry = 0;
    const char *at_platform = 0, *at_execfn = 0;
    for (; auxv[0]; auxv += 2) {
        switch (auxv[0]) {
        case 7: at_base = auxv[1]; break;
        case 3: at_phdr = auxv[1]; break;
        case 9: at_entry = auxv[1]; break;
        case 15: at_platform = (const char *)auxv[1]; break;
        case 23: at_secure = auxv[1]; break;
        case 25: at_random = auxv[1]; break;
        case 31: at_execfn = (const char *)auxv[1]; break;
        }
    }
    extern char _start[];
    check(at_base != 0, "auxv_at_base_nonzero");
    check(at_phdr != 0, "auxv_at_phdr");
    check(at_entry == (u64)_start, "auxv_at_entry_is_exe_start_after_aslr");
    check(at_secure == 0, "auxv_at_secure_zero");
    check(at_platform && seq(at_platform, "x86_64"), "auxv_at_platform");
    check(at_execfn && contains(at_execfn, "p3dyn"), "auxv_at_execfn");
    {
        const unsigned char *r = (const unsigned char *)at_random;
        int nz = 0;
        for (int i = 0; r && i < 16; i++) nz |= r[i];
        check(nz != 0, "auxv_at_random_nonzero");
    }

    /* DT_NEEDED chain + PLT + data relocations + constructors */
    check(p3a_add(2, 3) == 5, "needed_lib_plt_call");
    check(p3a_mulcall(6, 7) == 42, "needed_chain_and_data_reloc");
    check(p3a_ctor_flag() == 1, "needed_lib_constructor_ran");

    /* TLS: executable (local-exec) and DSO (general-dynamic, static slot) */
    check(exe_tls == 5 && exe_tbss == 0, "tls_exe_tdata_tbss");
    exe_tls = 6; exe_tbss = 7;
    check(exe_tls == 6 && exe_tbss == 7, "tls_exe_write");
    check(p3a_tls_get() == 41 && p3a_tbss_get() == 0, "tls_dso_initial");
    p3a_tls_set(77);
    check(p3a_tls_get() == 77 && p3a_tbss_get() == 78, "tls_dso_write");
    check(exe_tls == 6, "tls_modules_do_not_alias");

    /* MAP_FIXED replaces atomically, MAP_FIXED_NOREPLACE refuses */
    {
        s64 base = sc(9, 0, 8192, PROT_RW, MAP_PRIVATE_ANON, -1, 0);
        check(base > 0, "mmap_anon");
        volatile char *p = (volatile char *)base;
        p[0] = 'A'; p[4096] = 'B';
        s64 r = sc(9, base, 4096, PROT_RW, MAP_PRIVATE_ANON | MAP_FIXED, -1, 0);
        check(r == base, "mmap_fixed_returns_addr");
        check(p[0] == 0, "mmap_fixed_replaced_page_zeroed");
        check(p[4096] == 'B', "mmap_fixed_leaves_neighbour_page");
        s64 e = sc(9, base, 4096, PROT_RW, MAP_PRIVATE_ANON | MAP_FIXED_NOREPLACE, -1, 0);
        check(e == -17, "mmap_fixed_noreplace_eexist");
        check(sc(11, base, 8192, 0, 0, 0, 0) == 0, "munmap");
    }

    /* dlopen family served by ld-exo.so */
    void *h = dlopen("libp3c.so", 2 | 0x100);
    check(h != 0, "dlopen_libp3c");
    int (*fn)(void) = h ? (int (*)(void))dlsym(h, "p3c_value") : 0;
    check(fn && fn() == 1234, "dlsym_and_call");
    int (*tg)(void) = h ? (int (*)(void))dlsym(h, "p3c_tls_get") : 0;
    void (*ts)(int) = h ? (void (*)(int))dlsym(h, "p3c_tls_set") : 0;
    check(tg && tg() == 9, "tls_dlopened_dso_dynamic_dtv_initial");
    if (ts) ts(10);
    check(tg && tg() == 10, "tls_dlopened_dso_dynamic_dtv_write");
    check(p3a_tls_get() == 77, "tls_old_modules_intact_after_dtv_growth");
    check(dlsym((void *)0, "p3a_add") != 0, "dlsym_rtld_default_global");
    check(dlsym(h, "no_such_symbol_p3") == 0 && dlerror() != 0, "dlsym_missing_sets_dlerror");
    check(dlerror() == 0, "dlerror_cleared_after_read");
    check(dlopen("libnope_p3.so", 2) == 0 && dlerror() != 0, "dlopen_missing_sets_dlerror");
    {
        struct dl_info di = {0, 0, 0, 0};
        check(fn && dladdr((void *)fn, &di) && di.fname && contains(di.fname, "libp3c"), "dladdr_names_object");
    }
    phdr_count = 0;
    dl_iterate_phdr(phdr_cb, 0);
    check(phdr_count >= 5, "dl_iterate_phdr_lists_all_objects");
    check(h && dlclose(h) == 0, "dlclose_runs_destructor_and_unmaps");

    if (failures == 0) out("[P3_PROBE] ALL_PASS\n");
    else out("[P3_PROBE] FAILED\n");
    sc(231, failures ? 1 : 0, 0, 0, 0, 0, 0);
    for (;;) {}
}

__asm__(
    ".text\n"
    ".global _start\n"
    ".type _start,@function\n"
    "_start:\n"
    "    xor %ebp,%ebp\n"
    "    mov %rsp,%rdi\n"
    "    mov %rsp,%rsi\n"
    "    and $15,%rsi\n"
    "    and $-16,%rsp\n"
    "    call p3_main\n"
    "    hlt\n");
