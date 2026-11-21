/* Phase 3 probe: musl-exo static-PIE (rcrt1.o self-relocation, kernel ET_DYN + ASLR,
 * no PT_INTERP). Verifies %fs:0 TCB layout / per-thread errno, pthreads + TLS,
 * Linux openat/getdents64 numbering, MAP_FIXED. */
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static int failures;
static void check(int ok, const char *name)
{
    printf("[P3_MUSL] %s %s\n", ok ? "PASS" : "FAIL", name);
    if (!ok) failures++;
}

static __thread int tls_val = 100;
static __thread int tls_bss;

struct targ { int id; int errno_seen; int tls_seen; int bss_seen; };

static void *worker(void *p)
{
    struct targ *t = p;
    tls_val += t->id;
    tls_bss = t->id * 3;
    errno = 0;
    int fd = open("/definitely/not/here", O_RDONLY);
    int e = errno;
    for (volatile int i = 0; i < 20000; i++) {}   /* give other threads time to clobber */
    t->errno_seen = (fd < 0 && e == ENOENT && errno == ENOENT) ? 1 : 0;
    t->tls_seen = tls_val;
    t->bss_seen = tls_bss;
    return 0;
}

int main(int argc, char **argv)
{
    check(argc >= 1 && argv[0] && argv[0][0], "argv0");

    errno = 0;
    check(open("/nonexistent/p3", O_RDONLY) < 0 && errno == ENOENT, "open_enoent_sets_errno");
    errno = 1234;

    tls_val = 7;
    pthread_t th[4];
    struct targ ta[4];
    int created = 0;
    for (int i = 0; i < 4; i++) {
        ta[i].id = i + 1; ta[i].errno_seen = 0; ta[i].tls_seen = 0; ta[i].bss_seen = 0;
        if (pthread_create(&th[i], 0, worker, &ta[i]) == 0) created++;
    }
    check(created == 4, "pthread_create_4");
    for (int i = 0; i < created; i++) pthread_join(th[i], 0);
    int all_errno = 1, all_tls = 1;
    for (int i = 0; i < 4; i++) {
        all_errno &= ta[i].errno_seen;
        all_tls &= (ta[i].tls_seen == 100 + ta[i].id && ta[i].bss_seen == ta[i].id * 3);
    }
    check(all_errno, "errno_is_per_thread");
    check(all_tls, "tls_is_per_thread");
    check(errno == 1234 && tls_val == 7, "main_thread_errno_and_tls_untouched");

    DIR *d = opendir("/");
    int saw_bin = 0;
    if (d) {
        struct dirent *de;
        while ((de = readdir(d))) if (!strcmp(de->d_name, "bin")) saw_bin = 1;
        closedir(d);
    }
    check(d != 0 && saw_bin, "opendir_readdir_getdents64");

    unsigned char *p = mmap(0, 8192, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(p != MAP_FAILED, "mmap_anon");
    if (p != MAP_FAILED) {
        p[0] = 'A'; p[4096] = 'B';
        void *r = mmap(p, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
        check(r == p && p[0] == 0 && p[4096] == 'B', "mmap_fixed_replace");
        void *e = mmap(p, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
        check(e == MAP_FAILED && errno == EEXIST, "mmap_fixed_noreplace_eexist");
        check(munmap(p, 8192) == 0, "munmap");
    }

    printf(failures ? "[P3_MUSL] FAILED\n" : "[P3_MUSL] ALL_PASS\n");
    fflush(stdout);
    return failures ? 1 : 0;
}
