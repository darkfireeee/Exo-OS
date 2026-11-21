/* Phase 3 probe: DSO with DT_NEEDED chain (libp3b.so), .init_array constructor,
 * data relocations (function-pointer table) and general-dynamic TLS variables. */
extern int p3b_mul(int, int);

static int ctor_ran;
__thread int p3a_tls = 41;   /* .tdata */
__thread int p3a_tbss;       /* .tbss  */

static int (*const ops[])(int, int) = { p3b_mul };

int p3a_add(int a, int b) { return a + b; }
int p3a_mulcall(int a, int b) { return ops[0](a, b); }
int p3a_ctor_flag(void) { return ctor_ran; }
int p3a_tls_get(void) { return p3a_tls; }
int p3a_tbss_get(void) { return p3a_tbss; }
void p3a_tls_set(int v) { p3a_tls = v; p3a_tbss = v + 1; }

__attribute__((constructor)) static void p3a_ctor(void) { ctor_ran = 1; }
