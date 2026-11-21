/* Phase 3 probe: dlopen()ed DSO. Has TLS (dynamic DTV path after TLS sealing),
 * a constructor and a destructor that announce themselves on stdout. */
static long raw_write(const char *s, unsigned long n)
{
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "0"(1L), "D"(1L), "S"(s), "d"(n)
                     : "rcx", "r11", "memory");
    return ret;
}

__thread int p3c_tls = 9;

int p3c_value(void) { return 1234; }
int p3c_tls_get(void) { return p3c_tls; }
void p3c_tls_set(int v) { p3c_tls = v; }

__attribute__((constructor)) static void p3c_init(void) { raw_write("[p3c] init\n", 11); }
__attribute__((destructor)) static void p3c_fini(void) { raw_write("[p3c] fini\n", 11); }
