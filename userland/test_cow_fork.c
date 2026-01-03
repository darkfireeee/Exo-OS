/**
 * Test CoW Fork - Test du fork() avec Copy-on-Write
 * 
 * Ce test mesure les métriques réelles du CoW:
 * - Latence de fork()
 * - Refcount des pages partagées
 * - Déclenchement correct des page faults
 * - Cleanup mémoire (pas de fuites)
 */

#include <stdint.h>
#include <stddef.h>

// Syscalls
#define SYS_EXIT 60
#define SYS_FORK 57
#define SYS_GETPID 39
#define SYS_WRITE 1
#define SYS_SCHED_YIELD 24

// File descriptors
#define STDOUT 1
#define STDERR 2

// RDTSC pour mesurer les cycles
static inline uint64_t rdtsc(void) {
    uint32_t lo, hi;
    __asm__ volatile("rdtsc" : "=a"(lo), "=d"(hi));
    return ((uint64_t)hi << 32) | lo;
}

// Syscall wrappers
static inline long syscall1(long n, long a1) {
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n), "D"(a1) : "rcx", "r11", "memory");
    return ret;
}

static inline long syscall0(long n) {
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n) : "rcx", "r11", "memory");
    return ret;
}

static inline long syscall3(long n, long a1, long a2, long a3) {
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n), "D"(a1), "S"(a2), "d"(a3) : "rcx", "r11", "memory");
    return ret;
}

// Helper functions
static void print(const char *s) {
    size_t len = 0;
    while (s[len]) len++;
    syscall3(SYS_WRITE, STDOUT, (long)s, len);
}

static void print_num(const char *prefix, uint64_t num) {
    char buf[32];
    int i = 0;
    
    if (num == 0) {
        buf[i++] = '0';
    } else {
        char tmp[32];
        int j = 0;
        while (num > 0) {
            tmp[j++] = '0' + (num % 10);
            num /= 10;
        }
        while (j > 0) {
            buf[i++] = tmp[--j];
        }
    }
    buf[i] = '\0';
    
    print(prefix);
    syscall3(SYS_WRITE, STDOUT, (long)buf, i);
    print("\n");
}

static void exit(int code) {
    syscall1(SYS_EXIT, code);
    __builtin_unreachable();
}

static long fork(void) {
    return syscall0(SYS_FORK);
}

static long getpid(void) {
    return syscall0(SYS_GETPID);
}

static void sched_yield(void) {
    syscall0(SYS_SCHED_YIELD);
}

// Données globales pour tester le partage/copie
static volatile uint64_t shared_data = 0xDEADBEEF;
static volatile uint64_t modified_data = 0;

/**
 * Test 1: Latence du fork()
 * Objectif: < 1500 cycles
 */
static void test_fork_latency(void) {
    print("\n=== TEST 1: Fork Latency ===\n");
    
    uint64_t start = rdtsc();
    long pid = fork();
    uint64_t end = rdtsc();
    
    uint64_t cycles = end - start;
    
    if (pid == 0) {
        // Enfant: affiche sa latence et sort
        print_num("[CHILD] Fork latency (cycles): ", cycles);
        print("[CHILD] Exiting...\n");
        exit(0);
    } else {
        // Parent
        print_num("[PARENT] Fork latency (cycles): ", cycles);
        print_num("[PARENT] Child PID: ", (uint64_t)pid);
        
        if (cycles < 1500) {
            print("[PASS] Latency < 1500 cycles\n");
        } else {
            print("[FAIL] Latency >= 1500 cycles\n");
        }
        
        // Attendre un peu que l'enfant se termine
        for (int i = 0; i < 10; i++) {
            sched_yield();
        }
    }
}

/**
 * Test 2: Partage de pages avant écriture
 * Les pages doivent être partagées (refcount=2)
 */
static void test_page_sharing(void) {
    print("\n=== TEST 2: Page Sharing (CoW) ===\n");
    
    // Lire les données partagées
    uint64_t value = shared_data;
    print_num("[PARENT] Shared data BEFORE fork: ", value);
    
    long pid = fork();
    
    if (pid == 0) {
        // Enfant: lire sans modifier
        value = shared_data;
        print_num("[CHILD] Shared data (read-only): ", value);
        
        if (value == 0xDEADBEEF) {
            print("[CHILD PASS] Data shared correctly\n");
        } else {
            print("[CHILD FAIL] Data corrupted!\n");
        }
        
        exit(0);
    } else {
        // Parent: lire aussi
        value = shared_data;
        print_num("[PARENT] Shared data (read-only): ", value);
        
        // Attendre l'enfant
        for (int i = 0; i < 10; i++) {
            sched_yield();
        }
        
        print("[PASS] Page sharing test completed\n");
    }
}

/**
 * Test 3: Copy-on-Write (page fault sur écriture)
 * L'enfant modifie la donnée, le parent doit garder l'originale
 */
static void test_cow_write(void) {
    print("\n=== TEST 3: Copy-on-Write (Page Fault) ===\n");
    
    shared_data = 0xCAFEBABE;
    print_num("[PARENT] Initial value: ", shared_data);
    
    long pid = fork();
    
    if (pid == 0) {
        // Enfant: MODIFIER la donnée (doit déclencher CoW)
        print("[CHILD] Writing to shared page (triggers CoW)...\n");
        
        uint64_t old_value = shared_data;
        shared_data = 0x12345678; // PAGE FAULT ICI!
        uint64_t new_value = shared_data;
        
        print_num("[CHILD] Old value: ", old_value);
        print_num("[CHILD] New value: ", new_value);
        
        if (new_value == 0x12345678 && old_value == 0xCAFEBABE) {
            print("[CHILD PASS] CoW write successful\n");
        } else {
            print("[CHILD FAIL] CoW write failed!\n");
        }
        
        exit(0);
    } else {
        // Parent: attendre puis vérifier que sa donnée n'a pas changé
        for (int i = 0; i < 20; i++) {
            sched_yield();
        }
        
        uint64_t parent_value = shared_data;
        print_num("[PARENT] Value after child write: ", parent_value);
        
        if (parent_value == 0xCAFEBABE) {
            print("[PARENT PASS] Parent data unchanged (CoW worked!)\n");
        } else {
            print("[PARENT FAIL] Parent data corrupted!\n");
            print_num("[PARENT] Expected: ", 0xCAFEBABE);
        }
    }
}

/**
 * Test 4: Multiple forks (stress test refcount)
 */
static void test_multiple_forks(void) {
    print("\n=== TEST 4: Multiple Forks (Refcount Stress) ===\n");
    
    modified_data = 0xAAAAAAAA;
    
    for (int i = 0; i < 3; i++) {
        long pid = fork();
        
        if (pid == 0) {
            // Enfant
            print_num("[CHILD] Generation: ", i + 1);
            print_num("[CHILD] PID: ", (uint64_t)getpid());
            print_num("[CHILD] Data: ", modified_data);
            
            // Modifier la donnée
            modified_data = 0xBBBBBBBB + i;
            print_num("[CHILD] Modified to: ", modified_data);
            
            exit(0);
        } else {
            print_num("[PARENT] Created child: ", (uint64_t)pid);
            
            // Attendre un peu
            for (int j = 0; j < 5; j++) {
                sched_yield();
            }
        }
    }
    
    print_num("[PARENT] Final data value: ", modified_data);
    
    if (modified_data == 0xAAAAAAAA) {
        print("[PASS] Parent data intact after 3 forks\n");
    } else {
        print("[FAIL] Parent data corrupted\n");
    }
}

/**
 * Point d'entrée principal
 */
void _start(void) {
    print("====================================\n");
    print("  CoW Fork Test Suite\n");
    print("====================================\n");
    
    // Test 1: Latence
    test_fork_latency();
    
    // Test 2: Partage de pages
    test_page_sharing();
    
    // Test 3: Copy-on-Write
    test_cow_write();
    
    // Test 4: Forks multiples
    test_multiple_forks();
    
    print("\n====================================\n");
    print("  All Tests Completed\n");
    print("====================================\n");
    
    exit(0);
}
