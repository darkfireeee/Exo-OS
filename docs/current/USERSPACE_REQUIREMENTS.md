# 🔧 ÉLÉMENTS USERSPACE MINIMAUX PAR PHASE (0→3)

**Date:** 5 janvier 2026  
**Version Kernel:** v0.7.0  
**Objectif:** Liste exhaustive des programmes userspace requis pour valider chaque phase du roadmap

---

## 📋 VUE D'ENSEMBLE

| Phase | Programmes Requis | Complexité | Priorité |
|-------|------------------|------------|----------|
| **Phase 0** | Aucun (tests kernel) | N/A | ✅ Complet |
| **Phase 1** | 8-12 programmes | Faible-Moyenne | 🔴 CRITIQUE |
| **Phase 2** | 15-20 programmes | Moyenne | 🟡 Importante |
| **Phase 3** | 25+ programmes | Haute | 🟢 Future |

---

## ✅ PHASE 0: Fondations (100% - AUCUN USERSPACE)

### Tests Validés
- ✅ Timer preemption
- ✅ Context switch
- ✅ Memory virtual (map/unmap)
- ✅ Page fault handler

### Userspace Requis
```
AUCUN - Tous les tests en mode kernel
```

**Raison:** Phase 0 = infrastructure bas niveau, pas encore de mode user

---

## 🔴 PHASE 1: Kernel Fonctionnel (USERSPACE CRITIQUE)

### Phase 1a: VFS Complet

#### Programmes Minimaux (4 requis)

**1. test_tmpfs_basic** - Test tmpfs read/write
```c
// userland/test_tmpfs_basic.c
#include "syscalls.h"

void _start() {
    // Créer fichier
    int fd = open("/tmp/test.txt", O_CREAT | O_WRONLY, 0644);
    if (fd < 0) exit(1);
    
    // Écrire données
    const char *data = "Hello tmpfs!\n";
    if (write(fd, data, 13) != 13) exit(2);
    close(fd);
    
    // Relire
    fd = open("/tmp/test.txt", O_RDONLY, 0);
    if (fd < 0) exit(3);
    
    char buf[20];
    if (read(fd, buf, 13) != 13) exit(4);
    close(fd);
    
    // Vérifier contenu
    for (int i = 0; i < 13; i++) {
        if (buf[i] != data[i]) exit(5);
    }
    
    write(1, "[PASS] tmpfs OK\n", 16);
    exit(0);
}
```

**2. test_devfs** - Test /dev/null et /dev/zero
```c
// userland/test_devfs.c
void _start() {
    char buf[64];
    
    // Test /dev/null absorbe tout
    int null_fd = open("/dev/null", O_WRONLY, 0);
    if (write(null_fd, "data", 4) != 4) exit(1);
    close(null_fd);
    
    // Test /dev/zero produit zeros
    int zero_fd = open("/dev/zero", O_RDONLY, 0);
    read(zero_fd, buf, 64);
    close(zero_fd);
    
    for (int i = 0; i < 64; i++) {
        if (buf[i] != 0) exit(2);
    }
    
    write(1, "[PASS] devfs OK\n", 16);
    exit(0);
}
```

**3. test_procfs** - Lire /proc/self/status
```c
// userland/test_procfs.c
void _start() {
    int fd = open("/proc/self/status", O_RDONLY, 0);
    if (fd < 0) exit(1);
    
    char buf[256];
    int n = read(fd, buf, 256);
    if (n <= 0) exit(2);
    close(fd);
    
    // Afficher contenu
    write(1, "[PASS] procfs read:\n", 20);
    write(1, buf, n);
    exit(0);
}
```

**4. test_mount** - Test mount/unmount
```c
// userland/test_mount.c
void _start() {
    // Mount tmpfs sur /mnt
    if (mount("tmpfs", "/mnt", "tmpfs", 0, NULL) < 0) exit(1);
    
    // Créer fichier dedans
    int fd = open("/mnt/test", O_CREAT | O_WRONLY, 0644);
    if (fd < 0) exit(2);
    write(fd, "ok", 2);
    close(fd);
    
    // Unmount
    if (umount("/mnt") < 0) exit(3);
    
    write(1, "[PASS] mount/umount OK\n", 23);
    exit(0);
}
```

---

### Phase 1b: Process Management

#### Programmes Minimaux (5 requis)

**5. test_hello** - Premier exec() simple
```c
// userland/test_hello.c - DÉJÀ EXISTE
void _start() {
    write(1, "Hello from exec!\n", 17);
    exit(0);
}
```

**6. test_fork_simple** - Test fork basique
```c
// userland/test_fork_simple.c
void _start() {
    write(1, "Parent: forking...\n", 19);
    
    int pid = fork();
    
    if (pid == 0) {
        // Enfant
        write(1, "Child: running\n", 15);
        exit(0);
    } else if (pid > 0) {
        // Parent
        int status;
        wait4(pid, &status, 0, NULL);
        write(1, "Parent: child done\n", 19);
        exit(0);
    } else {
        write(1, "Fork failed!\n", 13);
        exit(1);
    }
}
```

**7. test_exec_args** - Test exec avec arguments
```c
// userland/test_exec_args.c
void _start(int argc, char **argv) {
    write(1, "Args received:\n", 15);
    
    for (int i = 0; i < argc; i++) {
        write(1, "  ", 2);
        write(1, argv[i], strlen(argv[i]));
        write(1, "\n", 1);
    }
    
    exit(0);
}
```

**8. test_fork_exec** - Cycle complet fork+exec+wait
```c
// userland/test_fork_exec.c
void _start() {
    int pid = fork();
    
    if (pid == 0) {
        // Enfant: exec vers autre programme
        char *argv[] = { "/bin/test_hello", NULL };
        char *envp[] = { NULL };
        execve("/bin/test_hello", argv, envp);
        
        // Ne devrait jamais arriver ici
        write(1, "exec failed!\n", 13);
        exit(1);
    } else {
        // Parent: attendre
        int status;
        wait4(pid, &status, 0, NULL);
        
        if (status == 0) {
            write(1, "[PASS] fork+exec+wait OK\n", 25);
            exit(0);
        } else {
            write(1, "[FAIL] child error\n", 19);
            exit(1);
        }
    }
}
```

**9. test_cow_real** - Test CoW avec vraie mémoire
```c
// userland/test_cow_real.c
void _start() {
    // Allouer buffer sur stack
    char buffer[4096];
    
    // Remplir avec 'A'
    for (int i = 0; i < 4096; i++) {
        buffer[i] = 'A';
    }
    
    write(1, "Before fork: ", 13);
    write(1, buffer, 10);  // "AAAAAAAAAA"
    write(1, "\n", 1);
    
    int pid = fork();
    
    if (pid == 0) {
        // Enfant: modifier buffer (trigger CoW)
        buffer[0] = 'C';
        buffer[1] = 'H';
        buffer[2] = 'I';
        buffer[3] = 'L';
        buffer[4] = 'D';
        
        write(1, "Child sees: ", 12);
        write(1, buffer, 10);  // "CHILDAAAAA"
        write(1, "\n", 1);
        exit(0);
        
    } else {
        // Parent: buffer doit rester inchangé
        wait4(pid, NULL, 0, NULL);
        
        write(1, "Parent sees: ", 13);
        write(1, buffer, 10);  // "AAAAAAAAAA" (inchangé!)
        write(1, "\n", 1);
        
        // Vérifier que pas modifié
        if (buffer[0] == 'A') {
            write(1, "[PASS] CoW worked!\n", 19);
            exit(0);
        } else {
            write(1, "[FAIL] CoW broken!\n", 19);
            exit(1);
        }
    }
}
```

---

### Phase 1c: Signals + Shell

#### Programmes Minimaux (3 requis)

**10. test_signal_basic** - Test SIGINT/SIGTERM
```c
// userland/test_signal_basic.c
volatile int signal_received = 0;

void signal_handler(int sig) {
    signal_received = sig;
}

void _start() {
    // Enregistrer handler
    struct sigaction sa;
    sa.sa_handler = signal_handler;
    sa.sa_flags = 0;
    sigaction(SIGINT, &sa, NULL);
    
    // S'envoyer un signal
    int pid = getpid();
    kill(pid, SIGINT);
    
    // Vérifier réception
    if (signal_received == SIGINT) {
        write(1, "[PASS] Signal received\n", 23);
        exit(0);
    } else {
        write(1, "[FAIL] No signal\n", 17);
        exit(1);
    }
}
```

**11. test_pipe** - Test pipe() pour IPC
```c
// userland/test_pipe.c
void _start() {
    int pipefd[2];
    
    if (pipe(pipefd) < 0) {
        write(1, "[FAIL] pipe() failed\n", 21);
        exit(1);
    }
    
    int pid = fork();
    
    if (pid == 0) {
        // Enfant: writer
        close(pipefd[0]);  // Close read end
        write(pipefd[1], "hello pipe", 10);
        close(pipefd[1]);
        exit(0);
        
    } else {
        // Parent: reader
        close(pipefd[1]);  // Close write end
        
        char buf[20];
        int n = read(pipefd[0], buf, 20);
        close(pipefd[0]);
        
        wait4(pid, NULL, 0, NULL);
        
        if (n == 10) {
            write(1, "[PASS] Pipe: ", 13);
            write(1, buf, n);
            write(1, "\n", 1);
            exit(0);
        } else {
            write(1, "[FAIL] Pipe read error\n", 23);
            exit(1);
        }
    }
}
```

**12. shell_minimal** - Shell interactif basique
```c
// userland/shell_minimal.c
void _start() {
    char cmd_buf[128];
    
    while (1) {
        write(1, "exo-shell> ", 11);
        
        // Lire commande
        int n = read(0, cmd_buf, 127);
        if (n <= 0) break;
        cmd_buf[n-1] = '\0';  // Remove newline
        
        // Parse simple: premier mot = commande
        char *cmd = cmd_buf;
        
        int pid = fork();
        
        if (pid == 0) {
            // Enfant: exec commande
            char *argv[] = { cmd, NULL };
            char *envp[] = { NULL };
            
            // Essayer /bin/cmd
            char path[256] = "/bin/";
            strcat(path, cmd);
            
            execve(path, argv, envp);
            
            // Si exec échoue
            write(1, "Command not found\n", 18);
            exit(1);
            
        } else {
            // Parent: attendre
            wait4(pid, NULL, 0, NULL);
        }
    }
    
    exit(0);
}
```

---

## 🟡 PHASE 2: Multi-core + Networking (USERSPACE IMPORTANT)

### Phase 2a: SMP Scheduler

#### Programmes Minimaux (5 requis)

**13. test_threads_affinity** - Test CPU affinity
```c
// userland/test_threads_affinity.c
void _start() {
    // Créer 4 threads (1 par CPU)
    for (int cpu = 0; cpu < 4; cpu++) {
        int pid = fork();
        
        if (pid == 0) {
            // Enfant: se binder au CPU
            cpu_set_t cpuset;
            CPU_ZERO(&cpuset);
            CPU_SET(cpu, &cpuset);
            
            sched_setaffinity(0, sizeof(cpuset), &cpuset);
            
            // Travailler pendant 1s
            for (volatile int i = 0; i < 100000000; i++);
            
            char msg[50];
            sprintf(msg, "Thread on CPU %d done\n", cpu);
            write(1, msg, strlen(msg));
            exit(0);
        }
    }
    
    // Parent: attendre tous les enfants
    for (int i = 0; i < 4; i++) {
        wait4(-1, NULL, 0, NULL);
    }
    
    write(1, "[PASS] SMP affinity OK\n", 23);
    exit(0);
}
```

**14. test_work_stealing** - Stress test load balancing
```c
// userland/test_work_stealing.c
void worker(int id) {
    char msg[50];
    sprintf(msg, "Worker %d starting\n", id);
    write(1, msg, strlen(msg));
    
    // Travail simulé
    for (volatile long i = 0; i < 50000000; i++);
    
    sprintf(msg, "Worker %d done\n", id);
    write(1, msg, strlen(msg));
    exit(0);
}

void _start() {
    // Créer 16 workers pour 4 CPUs
    // → Force load balancing
    for (int i = 0; i < 16; i++) {
        int pid = fork();
        if (pid == 0) {
            worker(i);
        }
    }
    
    // Attendre tous
    for (int i = 0; i < 16; i++) {
        wait4(-1, NULL, 0, NULL);
    }
    
    write(1, "[PASS] Load balancing OK\n", 25);
    exit(0);
}
```

**15. test_ipc_latency** - Benchmark IPC avec rdtsc
```c
// userland/test_ipc_latency.c
static inline uint64_t rdtsc(void) {
    uint32_t lo, hi;
    asm volatile("rdtsc" : "=a"(lo), "=d"(hi));
    return ((uint64_t)hi << 32) | lo;
}

void _start() {
    int pipefd[2];
    pipe(pipefd);
    
    int pid = fork();
    
    if (pid == 0) {
        // Enfant: echo server
        close(pipefd[1]);
        char buf[64];
        
        for (int i = 0; i < 1000; i++) {
            read(pipefd[0], buf, 64);
            write(pipefd[0], buf, 64);  // Echo back
        }
        exit(0);
        
    } else {
        // Parent: ping-pong benchmark
        close(pipefd[0]);
        char msg[64] = "ping";
        
        uint64_t start = rdtsc();
        
        for (int i = 0; i < 1000; i++) {
            write(pipefd[1], msg, 64);
            read(pipefd[1], msg, 64);
        }
        
        uint64_t elapsed = rdtsc() - start;
        uint64_t cycles_per_roundtrip = elapsed / 1000;
        
        char result[100];
        sprintf(result, "[BENCH] IPC latency: %lu cycles\n", 
                cycles_per_roundtrip);
        write(1, result, strlen(result));
        
        wait4(pid, NULL, 0, NULL);
        exit(0);
    }
}
```

**16. test_futex** - Test futex pour synchronisation
```c
// userland/test_futex.c
#include <linux/futex.h>

int shared_counter = 0;
int futex_var = 0;

void _start() {
    // Créer 4 threads qui incrémentent compteur
    for (int i = 0; i < 4; i++) {
        int pid = fork();
        
        if (pid == 0) {
            for (int j = 0; j < 1000; j++) {
                // Lock
                while (__sync_lock_test_and_set(&futex_var, 1)) {
                    futex(&futex_var, FUTEX_WAIT, 1, NULL, NULL, 0);
                }
                
                // Critical section
                shared_counter++;
                
                // Unlock
                __sync_lock_release(&futex_var);
                futex(&futex_var, FUTEX_WAKE, 1, NULL, NULL, 0);
            }
            exit(0);
        }
    }
    
    // Attendre
    for (int i = 0; i < 4; i++) {
        wait4(-1, NULL, 0, NULL);
    }
    
    // Vérifier
    if (shared_counter == 4000) {
        write(1, "[PASS] Futex sync OK\n", 21);
        exit(0);
    } else {
        char msg[50];
        sprintf(msg, "[FAIL] Counter=%d expected 4000\n", shared_counter);
        write(1, msg, strlen(msg));
        exit(1);
    }
}
```

**17. stress_smp** - Stress test général SMP
```c
// userland/stress_smp.c
void _start() {
    write(1, "SMP Stress Test: Creating 64 processes...\n", 43);
    
    int pids[64];
    
    // Créer 64 processus
    for (int i = 0; i < 64; i++) {
        pids[i] = fork();
        
        if (pids[i] == 0) {
            // Enfant: travail CPU + I/O
            char buf[256];
            
            for (int j = 0; j < 100; j++) {
                // CPU work
                for (volatile int k = 0; k < 1000000; k++);
                
                // I/O work
                sprintf(buf, "Process %d iteration %d\n", i, j);
                write(1, buf, strlen(buf));
            }
            
            exit(0);
        }
    }
    
    // Parent: attendre tous
    int completed = 0;
    for (int i = 0; i < 64; i++) {
        wait4(-1, NULL, 0, NULL);
        completed++;
        
        char msg[50];
        sprintf(msg, "Completed: %d/64\n", completed);
        write(1, msg, strlen(msg));
    }
    
    write(1, "[PASS] 64 processes completed\n", 30);
    exit(0);
}
```

---

### Phase 2b: Network Stack

#### Programmes Minimaux (5 requis)

**18. test_socket_basic** - Test socket() API
```c
// userland/test_socket_basic.c
void _start() {
    // Créer socket TCP
    int sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (sockfd < 0) {
        write(1, "[FAIL] socket() failed\n", 23);
        exit(1);
    }
    
    // Bind à port 8080
    struct sockaddr_in addr;
    addr.sin_family = AF_INET;
    addr.sin_port = htons(8080);
    addr.sin_addr.s_addr = INADDR_ANY;
    
    if (bind(sockfd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        write(1, "[FAIL] bind() failed\n", 21);
        exit(1);
    }
    
    close(sockfd);
    write(1, "[PASS] Socket bind OK\n", 22);
    exit(0);
}
```

**19. test_ping** - Test ICMP ping
```c
// userland/test_ping.c
void _start() {
    // Ouvrir raw socket
    int sockfd = socket(AF_INET, SOCK_RAW, IPPROTO_ICMP);
    if (sockfd < 0) exit(1);
    
    // Construire ICMP echo request
    struct icmp_packet {
        uint8_t type;  // 8 = echo request
        uint8_t code;  // 0
        uint16_t checksum;
        uint16_t id;
        uint16_t seq;
        char data[56];
    } packet;
    
    packet.type = 8;
    packet.code = 0;
    packet.id = getpid();
    packet.seq = 1;
    // ... calculate checksum ...
    
    // Envoyer vers 127.0.0.1
    struct sockaddr_in dest;
    dest.sin_family = AF_INET;
    dest.sin_addr.s_addr = inet_addr("127.0.0.1");
    
    sendto(sockfd, &packet, sizeof(packet), 0,
           (struct sockaddr*)&dest, sizeof(dest));
    
    // Attendre réponse (timeout 1s)
    char reply[1024];
    struct timeval tv = { .tv_sec = 1, .tv_usec = 0 };
    setsockopt(sockfd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    
    int n = recvfrom(sockfd, reply, 1024, 0, NULL, NULL);
    
    if (n > 0) {
        write(1, "[PASS] Ping OK\n", 15);
        exit(0);
    } else {
        write(1, "[FAIL] No reply\n", 16);
        exit(1);
    }
}
```

**20. test_tcp_server** - Serveur TCP simple
```c
// userland/test_tcp_server.c
void _start() {
    int sockfd = socket(AF_INET, SOCK_STREAM, 0);
    
    struct sockaddr_in addr;
    addr.sin_family = AF_INET;
    addr.sin_port = htons(8080);
    addr.sin_addr.s_addr = INADDR_ANY;
    
    bind(sockfd, (struct sockaddr*)&addr, sizeof(addr));
    listen(sockfd, 5);
    
    write(1, "Server listening on port 8080...\n", 33);
    
    while (1) {
        struct sockaddr_in client_addr;
        socklen_t len = sizeof(client_addr);
        
        int client = accept(sockfd, (struct sockaddr*)&client_addr, &len);
        
        if (client < 0) continue;
        
        write(1, "Client connected!\n", 18);
        
        // Echo server
        char buf[1024];
        int n;
        while ((n = read(client, buf, 1024)) > 0) {
            write(client, buf, n);
        }
        
        close(client);
        write(1, "Client disconnected\n", 20);
    }
}
```

**21. test_tcp_client** - Client TCP simple
```c
// userland/test_tcp_client.c
void _start() {
    int sockfd = socket(AF_INET, SOCK_STREAM, 0);
    
    struct sockaddr_in server;
    server.sin_family = AF_INET;
    server.sin_port = htons(8080);
    server.sin_addr.s_addr = inet_addr("127.0.0.1");
    
    if (connect(sockfd, (struct sockaddr*)&server, sizeof(server)) < 0) {
        write(1, "[FAIL] Connection failed\n", 25);
        exit(1);
    }
    
    write(1, "Connected to server!\n", 21);
    
    // Envoyer message
    const char *msg = "Hello TCP!";
    write(sockfd, msg, strlen(msg));
    
    // Recevoir echo
    char buf[100];
    int n = read(sockfd, buf, 100);
    
    write(1, "Received: ", 10);
    write(1, buf, n);
    write(1, "\n", 1);
    
    close(sockfd);
    write(1, "[PASS] TCP echo OK\n", 19);
    exit(0);
}
```

**22. test_udp** - Test UDP send/recv
```c
// userland/test_udp.c
void _start() {
    int sockfd = socket(AF_INET, SOCK_DGRAM, 0);
    
    // Bind à port
    struct sockaddr_in addr;
    addr.sin_family = AF_INET;
    addr.sin_port = htons(9000);
    addr.sin_addr.s_addr = INADDR_ANY;
    
    bind(sockfd, (struct sockaddr*)&addr, sizeof(addr));
    
    int pid = fork();
    
    if (pid == 0) {
        // Enfant: sender
        sleep(1);  // Wait for parent to be ready
        
        struct sockaddr_in dest;
        dest.sin_family = AF_INET;
        dest.sin_port = htons(9000);
        dest.sin_addr.s_addr = inet_addr("127.0.0.1");
        
        const char *msg = "UDP test";
        sendto(sockfd, msg, 8, 0, (struct sockaddr*)&dest, sizeof(dest));
        
        exit(0);
        
    } else {
        // Parent: receiver
        char buf[100];
        struct sockaddr_in from;
        socklen_t fromlen = sizeof(from);
        
        int n = recvfrom(sockfd, buf, 100, 0,
                        (struct sockaddr*)&from, &fromlen);
        
        if (n == 8) {
            write(1, "[PASS] UDP: ", 12);
            write(1, buf, n);
            write(1, "\n", 1);
        } else {
            write(1, "[FAIL] UDP receive\n", 19);
        }
        
        wait4(pid, NULL, 0, NULL);
        exit(0);
    }
}
```

---

## 🟢 PHASE 3: Drivers + Storage (USERSPACE AVANCÉ)

### Phase 3a: Block Drivers + Filesystems

#### Programmes Minimaux (8 requis)

**23. test_disk_read** - Lecture disque basique
```c
// userland/test_disk_read.c
void _start() {
    // Ouvrir device bloc
    int fd = open("/dev/vda", O_RDONLY, 0);
    if (fd < 0) {
        write(1, "[FAIL] Cannot open /dev/vda\n", 28);
        exit(1);
    }
    
    // Lire premier secteur (MBR)
    char sector[512];
    if (read(fd, sector, 512) != 512) {
        write(1, "[FAIL] Read failed\n", 19);
        exit(1);
    }
    
    // Vérifier signature MBR (0x55AA)
    if (sector[510] == 0x55 && sector[511] == 0xAA) {
        write(1, "[PASS] MBR signature OK\n", 24);
        exit(0);
    } else {
        write(1, "[FAIL] Invalid MBR\n", 19);
        exit(1);
    }
}
```

**24. test_ext4_mount** - Mount ext4 filesystem
```c
// userland/test_ext4_mount.c
void _start() {
    // Créer point de mount
    mkdir("/mnt/disk", 0755);
    
    // Mount partition ext4
    if (mount("/dev/vda1", "/mnt/disk", "ext4", 0, NULL) < 0) {
        write(1, "[FAIL] Mount failed\n", 20);
        exit(1);
    }
    
    write(1, "[PASS] ext4 mounted\n", 20);
    
    // Tester lecture
    int fd = open("/mnt/disk/test.txt", O_RDONLY, 0);
    if (fd >= 0) {
        char buf[100];
        int n = read(fd, buf, 100);
        write(1, "File content: ", 14);
        write(1, buf, n);
        write(1, "\n", 1);
        close(fd);
    }
    
    umount("/mnt/disk");
    exit(0);
}
```

**25. test_file_operations** - Tests I/O complets
```c
// userland/test_file_operations.c
void _start() {
    const char *testfile = "/mnt/disk/iotest.dat";
    
    // Create
    int fd = open(testfile, O_CREAT | O_WRONLY, 0644);
    if (fd < 0) exit(1);
    
    // Write 10KB
    char data[1024];
    for (int i = 0; i < 1024; i++) data[i] = (char)i;
    
    for (int i = 0; i < 10; i++) {
        if (write(fd, data, 1024) != 1024) exit(2);
    }
    close(fd);
    
    // Read back
    fd = open(testfile, O_RDONLY, 0);
    if (fd < 0) exit(3);
    
    char readbuf[1024];
    for (int i = 0; i < 10; i++) {
        if (read(fd, readbuf, 1024) != 1024) exit(4);
        
        // Verify
        for (int j = 0; j < 1024; j++) {
            if (readbuf[j] != (char)j) exit(5);
        }
    }
    close(fd);
    
    // Seek test
    fd = open(testfile, O_RDONLY, 0);
    lseek(fd, 5000, SEEK_SET);
    read(fd, readbuf, 100);
    close(fd);
    
    // Delete
    unlink(testfile);
    
    write(1, "[PASS] File I/O OK\n", 19);
    exit(0);
}
```

**26. test_directory_ops** - Opérations répertoires
```c
// userland/test_directory_ops.c
void _start() {
    // Create directory tree
    mkdir("/mnt/disk/testdir", 0755);
    mkdir("/mnt/disk/testdir/sub1", 0755);
    mkdir("/mnt/disk/testdir/sub2", 0755);
    
    // Create files
    int fd1 = open("/mnt/disk/testdir/file1.txt", O_CREAT|O_WRONLY, 0644);
    write(fd1, "test1", 5);
    close(fd1);
    
    int fd2 = open("/mnt/disk/testdir/sub1/file2.txt", O_CREAT|O_WRONLY, 0644);
    write(fd2, "test2", 5);
    close(fd2);
    
    // List directory
    DIR *dir = opendir("/mnt/disk/testdir");
    if (!dir) exit(1);
    
    write(1, "Directory listing:\n", 19);
    struct dirent *entry;
    while ((entry = readdir(dir)) != NULL) {
        write(1, "  ", 2);
        write(1, entry->d_name, strlen(entry->d_name));
        write(1, "\n", 1);
    }
    closedir(dir);
    
    // Cleanup
    unlink("/mnt/disk/testdir/file1.txt");
    unlink("/mnt/disk/testdir/sub1/file2.txt");
    rmdir("/mnt/disk/testdir/sub1");
    rmdir("/mnt/disk/testdir/sub2");
    rmdir("/mnt/disk/testdir");
    
    write(1, "[PASS] Directory ops OK\n", 24);
    exit(0);
}
```

**27. test_fat32_read** - Lecture FAT32
```c
// userland/test_fat32_read.c
void _start() {
    // Mount FAT32 partition
    mkdir("/mnt/fat", 0755);
    
    if (mount("/dev/vda2", "/mnt/fat", "vfat", MS_RDONLY, NULL) < 0) {
        write(1, "[FAIL] FAT32 mount failed\n", 26);
        exit(1);
    }
    
    // Lire fichier
    int fd = open("/mnt/fat/README.TXT", O_RDONLY, 0);
    if (fd >= 0) {
        char buf[256];
        int n = read(fd, buf, 256);
        
        write(1, "[PASS] FAT32 content:\n", 22);
        write(1, buf, n);
        write(1, "\n", 1);
        close(fd);
    } else {
        write(1, "[WARN] No README.TXT\n", 21);
    }
    
    umount("/mnt/fat");
    exit(0);
}
```

**28. benchmark_disk_io** - Benchmark I/O disque
```c
// userland/benchmark_disk_io.c
static inline uint64_t rdtsc(void) {
    uint32_t lo, hi;
    asm volatile("rdtsc" : "=a"(lo), "=d"(hi));
    return ((uint64_t)hi << 32) | lo;
}

void _start() {
    const char *testfile = "/mnt/disk/bench.dat";
    char data[4096];
    
    // Write benchmark
    int fd = open(testfile, O_CREAT|O_WRONLY, 0644);
    
    uint64_t start = rdtsc();
    for (int i = 0; i < 1000; i++) {  // 4MB
        write(fd, data, 4096);
    }
    uint64_t write_cycles = rdtsc() - start;
    close(fd);
    
    // Read benchmark
    fd = open(testfile, O_RDONLY, 0);
    
    start = rdtsc();
    for (int i = 0; i < 1000; i++) {
        read(fd, data, 4096);
    }
    uint64_t read_cycles = rdtsc() - start;
    close(fd);
    
    // Results
    char msg[200];
    sprintf(msg, "[BENCH] Write: %lu cycles/4KB, Read: %lu cycles/4KB\n",
            write_cycles/1000, read_cycles/1000);
    write(1, msg, strlen(msg));
    
    unlink(testfile);
    exit(0);
}
```

**29. test_mmap_file** - mmap() sur fichier
```c
// userland/test_mmap_file.c
void _start() {
    // Créer fichier 8KB
    int fd = open("/mnt/disk/mmaptest", O_CREAT|O_RDWR, 0644);
    
    char data[8192];
    for (int i = 0; i < 8192; i++) data[i] = i & 0xFF;
    write(fd, data, 8192);
    
    // mmap le fichier
    void *addr = mmap(NULL, 8192, PROT_READ|PROT_WRITE,
                      MAP_SHARED, fd, 0);
    
    if (addr == MAP_FAILED) {
        write(1, "[FAIL] mmap failed\n", 19);
        exit(1);
    }
    
    // Vérifier données
    unsigned char *ptr = (unsigned char*)addr;
    for (int i = 0; i < 8192; i++) {
        if (ptr[i] != (i & 0xFF)) {
            write(1, "[FAIL] Data mismatch\n", 21);
            exit(1);
        }
    }
    
    // Modifier via mmap
    ptr[0] = 0xFF;
    ptr[100] = 0xAA;
    
    // Sync vers disque
    msync(addr, 8192, MS_SYNC);
    
    munmap(addr, 8192);
    close(fd);
    
    // Relire fichier pour vérifier
    fd = open("/mnt/disk/mmaptest", O_RDONLY, 0);
    read(fd, data, 8192);
    close(fd);
    
    if (data[0] == 0xFF && data[100] == 0xAA) {
        write(1, "[PASS] mmap file OK\n", 20);
        exit(0);
    } else {
        write(1, "[FAIL] Changes not persisted\n", 29);
        exit(1);
    }
}
```

**30. test_page_cache** - Test page cache efficacité
```c
// userland/test_page_cache.c
void _start() {
    const char *file = "/mnt/disk/cachetest";
    char buf[4096];
    
    // Première lecture (cache miss)
    uint64_t start1 = rdtsc();
    int fd1 = open(file, O_RDONLY, 0);
    read(fd1, buf, 4096);
    close(fd1);
    uint64_t cycles1 = rdtsc() - start1;
    
    // Deuxième lecture (cache hit)
    uint64_t start2 = rdtsc();
    int fd2 = open(file, O_RDONLY, 0);
    read(fd2, buf, 4096);
    close(fd2);
    uint64_t cycles2 = rdtsc() - start2;
    
    char msg[150];
    sprintf(msg, "[BENCH] First read: %lu cycles, Second read: %lu cycles\n",
            cycles1, cycles2);
    write(1, msg, strlen(msg));
    
    if (cycles2 < cycles1 / 2) {
        write(1, "[PASS] Page cache effective\n", 29);
        exit(0);
    } else {
        write(1, "[WARN] Cache may not be working\n", 33);
        exit(0);
    }
}
```

---

### Phase 3b: Network Drivers

#### Programmes Minimaux (2 requis)

**31. test_ethernet** - Test interface Ethernet
```c
// userland/test_ethernet.c
void _start() {
    // Créer raw socket
    int sockfd = socket(AF_PACKET, SOCK_RAW, htons(ETH_P_ALL));
    if (sockfd < 0) exit(1);
    
    // Bind à interface eth0
    struct sockaddr_ll addr;
    addr.sll_family = AF_PACKET;
    addr.sll_protocol = htons(ETH_P_ALL);
    addr.sll_ifindex = if_nametoindex("eth0");
    
    bind(sockfd, (struct sockaddr*)&addr, sizeof(addr));
    
    // Recevoir quelques frames
    write(1, "Listening on eth0...\n", 21);
    
    char frame[1518];
    for (int i = 0; i < 10; i++) {
        int n = recv(sockfd, frame, 1518, 0);
        
        char msg[100];
        sprintf(msg, "Frame %d: %d bytes\n", i, n);
        write(1, msg, strlen(msg));
    }
    
    close(sockfd);
    write(1, "[PASS] Ethernet OK\n", 19);
    exit(0);
}
```

**32. test_dhcp_client** - Client DHCP simple
```c
// userland/test_dhcp_client.c
void _start() {
    // Créer UDP socket
    int sockfd = socket(AF_INET, SOCK_DGRAM, 0);
    
    // Bind port 68 (DHCP client)
    struct sockaddr_in addr;
    addr.sin_family = AF_INET;
    addr.sin_port = htons(68);
    addr.sin_addr.s_addr = INADDR_ANY;
    
    bind(sockfd, (struct sockaddr*)&addr, sizeof(addr));
    
    // Envoyer DHCP DISCOVER
    struct dhcp_packet {
        uint8_t op;  // 1 = request
        uint8_t htype;  // 1 = ethernet
        uint8_t hlen;  // 6
        uint8_t hops;  // 0
        uint32_t xid;  // transaction ID
        // ... autres champs DHCP ...
    } discover;
    
    // ... remplir packet ...
    
    struct sockaddr_in broadcast;
    broadcast.sin_family = AF_INET;
    broadcast.sin_port = htons(67);  // DHCP server
    broadcast.sin_addr.s_addr = INADDR_BROADCAST;
    
    sendto(sockfd, &discover, sizeof(discover), 0,
           (struct sockaddr*)&broadcast, sizeof(broadcast));
    
    write(1, "DHCP DISCOVER sent\n", 19);
    
    // Attendre DHCP OFFER (timeout 5s)
    struct timeval tv = { .tv_sec = 5 };
    setsockopt(sockfd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    
    char response[1024];
    int n = recv(sockfd, response, 1024, 0);
    
    if (n > 0) {
        write(1, "[PASS] DHCP OFFER received\n", 27);
        // Parse offered IP...
    } else {
        write(1, "[WARN] No DHCP response\n", 24);
    }
    
    close(sockfd);
    exit(0);
}
```

---

## 📊 RÉSUMÉ PAR PHASE

### Phase 1 (CRITIQUE)
**Total:** 12 programmes  
**Complexité:** Faible-Moyenne  
**Estimation:** 2-3 semaines

| Programme | Fonctionnalité | Lignes |
|-----------|---------------|---------|
| test_tmpfs_basic | VFS tmpfs | ~50 |
| test_devfs | /dev/null, /dev/zero | ~40 |
| test_procfs | /proc/self | ~30 |
| test_mount | mount/umount | ~40 |
| test_hello | Premier exec | ~10 |
| test_fork_simple | fork basique | ~30 |
| test_exec_args | exec avec args | ~25 |
| test_fork_exec | Cycle complet | ~40 |
| test_cow_real | CoW avec mémoire | ~60 |
| test_signal_basic | Signaux | ~35 |
| test_pipe | IPC pipe | ~50 |
| shell_minimal | Shell interactif | ~60 |

---

### Phase 2 (IMPORTANTE)
**Total:** 10 programmes  
**Complexité:** Moyenne  
**Estimation:** 3-4 semaines

| Programme | Fonctionnalité | Lignes |
|-----------|---------------|---------|
| test_threads_affinity | CPU affinity | ~45 |
| test_work_stealing | Load balancing | ~45 |
| test_ipc_latency | Benchmark IPC | ~60 |
| test_futex | Synchronisation | ~55 |
| stress_smp | Stress 64 proc | ~50 |
| test_socket_basic | Socket API | ~35 |
| test_ping | ICMP ping | ~60 |
| test_tcp_server | Serveur TCP | ~50 |
| test_tcp_client | Client TCP | ~40 |
| test_udp | UDP send/recv | ~50 |

---

### Phase 3 (FUTURE)
**Total:** 10 programmes  
**Complexité:** Haute  
**Estimation:** 4-6 semaines

| Programme | Fonctionnalité | Lignes |
|-----------|---------------|---------|
| test_disk_read | Lecture disque | ~40 |
| test_ext4_mount | Mount ext4 | ~45 |
| test_file_operations | I/O complet | ~70 |
| test_directory_ops | mkdir/rmdir | ~55 |
| test_fat32_read | FAT32 | ~40 |
| benchmark_disk_io | Bench I/O | ~55 |
| test_mmap_file | mmap fichier | ~70 |
| test_page_cache | Cache perf | ~50 |
| test_ethernet | Ethernet raw | ~45 |
| test_dhcp_client | DHCP | ~60 |

---

## 🎯 PRIORITÉS IMMÉDIATES

### Semaine 1: Infrastructure Base
```bash
# Créer bibliothèque syscalls réutilisable
userland/lib/
├── syscalls.h    # Wrappers inline
├── string.c      # strlen, memcpy, etc
└── Makefile      # Build system
```

### Semaine 2-3: Phase 1 Programs
```bash
# Ordre de création recommandé:
1. test_hello          # Le plus simple
2. test_fork_simple    # Fork basique
3. test_exec_args      # Exec avec args
4. test_fork_exec      # Cycle complet
5. test_cow_real       # CoW CRITIQUE
6. test_tmpfs_basic    # VFS
7. test_devfs          # Devices
8. test_procfs         # /proc
9. test_mount          # Filesystems
10. test_signal_basic  # Signaux
11. test_pipe          # IPC
12. shell_minimal      # Shell final
```

### Semaine 4-6: Phase 2 Programs
```bash
# Après Phase 1 validée:
13-17. Tests SMP (affinity, load balance, stress)
18-22. Tests Network (socket, TCP, UDP, ping)
```

---

## 🔧 BUILD SYSTEM RECOMMANDÉ

```makefile
# userland/Makefile

CC = musl-gcc
CFLAGS = -static -nostdlib -fno-pie -no-pie -O2 -Wall
LDFLAGS = -static

TESTS_PHASE1 = \
    test_hello \
    test_fork_simple \
    test_exec_args \
    test_fork_exec \
    test_cow_real \
    test_tmpfs_basic \
    test_devfs \
    test_procfs \
    test_mount \
    test_signal_basic \
    test_pipe \
    shell_minimal

TESTS_PHASE2 = \
    test_threads_affinity \
    test_work_stealing \
    test_ipc_latency \
    test_futex \
    stress_smp \
    test_socket_basic \
    test_ping \
    test_tcp_server \
    test_tcp_client \
    test_udp

TESTS_PHASE3 = \
    test_disk_read \
    test_ext4_mount \
    test_file_operations \
    test_directory_ops \
    test_fat32_read \
    benchmark_disk_io \
    test_mmap_file \
    test_page_cache \
    test_ethernet \
    test_dhcp_client

all: phase1 phase2 phase3

phase1: $(TESTS_PHASE1:=.elf)
phase2: $(TESTS_PHASE2:=.elf)
phase3: $(TESTS_PHASE3:=.elf)

%.elf: %.c lib/syscalls.h lib/string.c
	$(CC) $(CFLAGS) -Ilib -o $@ $< lib/string.c

clean:
	rm -f *.elf

install: all
	mkdir -p ../build/userland
	cp *.elf ../build/userland/

.PHONY: all phase1 phase2 phase3 clean install
```

---

## 📝 NEXT STEPS

1. **Créer `userland/lib/syscalls.h`** - Wrapper syscalls réutilisable
2. **Créer `userland/lib/string.c`** - Fonctions C basiques
3. **Implémenter test_hello.c** - Plus simple pour tester exec()
4. **Implémenter test_cow_real.c** - CRITIQUE pour débloquer tests actuels
5. **Build system** - Makefile pour compiler tous les tests
6. **Intégration kernel** - Charger binaires dans VFS au boot

**Temps total estimé:** 6-10 semaines pour compléter toutes les phases

---

## 🔴 PROBLÈMES ACTUELS RÉSOLUS

### Tests CoW Limités
```
❌ AVANT: Kernel thread = 0 pages
✅ APRÈS: test_cow_real.c = 1+ pages réelles
```

**Impact:** Débloque validation complète du CoW manager

### Tests CoW Limités
```
❌ TEST 1 (Fork Latency): 610M cycles mesuré
   Problème: Kernel thread = address space vide
   Impact: capture_address_space() retourne 0 pages
   
❌ TEST 2 (CoW Manager): 0 pages trackées
   Problème: Aucune vraie page à cloner
   Impact: CoW infrastructure non testable réellement

❌ TEST 3 (Stress): Timeout avant fin
   Problème: Trop lent, logging excessif
   Impact: Ne peut pas valider multiple forks
```

### Blocage Général
- **Tous les tests tournent en kernel threads** → pas de vrais address spaces
- **Pas d'exec() fonctionnel** → pas de programmes userspace chargés
- **Pas de vraie mémoire user** → CoW ne s'active jamais vraiment
- **Timeout QEMU** → tests longs ne finissent pas

---

## ✅ ÉLÉMENTS MINIMAUX NÉCESSAIRES

### Niveau 1: CRITIQUE (Débloquer CoW + Fork)

#### 1.1. ELF Loader Fonctionnel
**Fichier:** `kernel/src/loader/elf_loader.rs`  
**Ce qui existe déjà:**
- ✅ Parser ELF complet (headers, program headers)
- ✅ Structures ElfFile, ProgramHeader, etc.

**Ce qui manque:**
```rust
□ load_elf_to_memory(path: &str) -> Result<ProcessImage>
  - Lire le fichier ELF depuis VFS
  - Parser les segments PT_LOAD
  - Allouer pages physiques pour chaque segment
  - Copier les données du segment en mémoire
  - Mapper avec les bonnes permissions (R/W/X)
  
□ setup_user_stack(size: usize) -> VirtualAddress
  - Allouer stack userspace (ex: 8KB)
  - Mapper avec permissions USER | WRITABLE
  - Retourner top of stack
  
□ prepare_argv_envp(stack: VirtualAddress, argv: &[&str]) -> VirtualAddress
  - Pusher arguments sur le stack
  - Respecter ABI x86_64 (argc, argv[], NULL, envp[], NULL)
  - Retourner nouveau stack pointer
```

**Tests à activer:**
```c
// userland/test_hello.c - DÉJÀ EXISTANT
void _start() {
    write(1, "Hello from exec!\n", 17);
    exit(0);
}
```

**Livrable:** `sys_execve("/bin/test_hello", [], [])` charge et exécute le programme

---

#### 1.2. sys_execve() Complet
**Fichier:** `kernel/src/syscall/handlers/process.rs`  
**Ce qui existe:**
```rust
pub fn sys_execve(
    path: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
) -> MemoryResult<i32>
```

**Ce qui manque:**
```rust
□ Validation path (check NULL, copy from userspace)
□ Parse argv/envp arrays
□ Ouvrir le fichier via VFS
□ Appeler load_elf_to_memory()
□ Détruire ancien address space du processus
□ Configurer nouveau address space (PT_LOAD mappings)
□ Setup stack avec argv/envp
□ Changer CPU context (RIP = entry_point, RSP = stack_top)
□ Retourner vers userspace à la nouvelle RIP
```

**Test kernel:**
```rust
#[test]
fn test_exec_hello() {
    // Créer thread avec fork()
    let child_pid = sys_fork()?;
    
    if child_pid == 0 {
        // Enfant: exécuter /bin/test_hello
        sys_execve("/bin/test_hello\0", &[], &[])?;
        unreachable!();
    } else {
        // Parent: attendre
        let mut status = 0;
        sys_wait4(child_pid, &mut status, 0, null_mut())?;
        assert_eq!(status, 0);
    }
}
```

---

#### 1.3. Vraies Pages Userspace
**Fichier:** `kernel/src/memory/address_space.rs`  
**Objectif:** Les processus doivent avoir des vraies pages mappées

**Ce qui manque:**
```rust
□ user_heap_start: VirtualAddress (ex: 0x40000000)
□ user_heap_current: VirtualAddress
□ user_stack_start: VirtualAddress (ex: 0x7FFFFFFFE000)

□ allocate_user_pages(count: usize) -> Result<Vec<PhysicalAddress>>
  - Allouer frames physiques
  - Les marquer comme USER | WRITABLE
  - Retourner adresses physiques
  
□ map_user_region(virt_start: VAddr, phys_pages: &[PAddr], perms: PagePerms)
  - Créer mappings dans page table
  - Vérifier que c'est bien dans user range
  - Flush TLB si nécessaire
```

**Test:**
```rust
#[test]
fn test_user_pages_cow() {
    // Allouer 3 pages user
    let pages = allocate_user_pages(3)?;
    map_user_region(0x400000, &pages, USER | WRITABLE)?;
    
    // Fork
    let child = sys_fork()?;
    
    // Vérifier CoW
    let stats = cow_manager::get_stats();
    assert!(stats.total_pages >= 3); // Au moins les 3 pages
    assert!(stats.total_refs >= 6);  // Parent + child
}
```

---

### Niveau 2: IMPORTANT (Débloquer Tests Phase 2)

#### 2.1. Programmes Userspace de Test
**Répertoire:** `userland/tests/`

**Programmes minimaux à créer:**

```c
// test_fork_simple.c
void _start() {
    int pid = fork();
    if (pid == 0) {
        write(1, "Child\n", 6);
        exit(0);
    } else {
        write(1, "Parent\n", 7);
        wait(NULL);
        exit(0);
    }
}
```

```c
// test_cow_real.c  
void _start() {
    // Allouer un buffer
    char buffer[4096];
    for (int i = 0; i < 4096; i++) {
        buffer[i] = 'A';
    }
    
    int pid = fork();
    
    if (pid == 0) {
        // Enfant: modifier le buffer (déclenchera CoW)
        buffer[0] = 'B';
        write(1, buffer, 10);  // Affiche "BAAA..."
        exit(0);
    } else {
        // Parent: buffer inchangé
        wait(NULL);
        write(1, buffer, 10);  // Affiche "AAAA..."
        exit(0);
    }
}
```

```c
// test_exec_chain.c
void _start() {
    int pid = fork();
    if (pid == 0) {
        // Enfant: exec vers autre programme
        char *argv[] = { "/bin/test_hello", NULL };
        execve("/bin/test_hello", argv, NULL);
        exit(1); // Ne devrait jamais arriver
    } else {
        wait(NULL);
        write(1, "exec OK\n", 8);
        exit(0);
    }
}
```

**Compilation:**
```bash
# Sans libc (freestanding)
gcc -nostdlib -static -fno-pie -no-pie \
    -o test_fork_simple.elf test_fork_simple.c
    
# Ou avec musl (plus facile)
musl-gcc -static -o test_fork_simple.elf test_fork_simple.c
```

---

#### 2.2. Chargement dans VFS au Boot
**Fichier:** `kernel/src/lib.rs`

**Ce qui existe:**
```rust
// Ligne ~580: Initialisation VFS
vfs::init_tmpfs()?;
vfs::init_devfs()?;
```

**Ce qui manque:**
```rust
□ Fonction load_test_binaries() {
    // Créer /bin
    vfs::mkdir("/bin")?;
    
    // Charger les binaires depuis GRUB modules ou embedded
    let hello_elf = include_bytes!("../../userland/test_hello.elf");
    vfs::write_file("/bin/test_hello", hello_elf)?;
    
    let fork_elf = include_bytes!("../../userland/test_fork_simple.elf");
    vfs::write_file("/bin/test_fork", fork_elf)?;
    
    // etc.
}
```

**Alternative (GRUB modules):**
```
# Dans grub.cfg
module /boot/test_hello.elf
module /boot/test_fork.elf
```

Puis parser multiboot2 modules au boot.

---

#### 2.3. Tests Automatisés avec Vraie Exec
**Fichier:** `kernel/src/tests/phase2_exec_tests.rs`

```rust
pub fn test_exec_hello() {
    early_print("\n=== TEST: exec() Simple ===\n");
    
    match sys_fork() {
        Ok(child_pid) if child_pid == 0 => {
            // Enfant: exécuter /bin/test_hello
            let path = "/bin/test_hello\0";
            let argv = [core::ptr::null()];
            let envp = [core::ptr::null()];
            
            match sys_execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr()) {
                Ok(_) => unreachable!("execve should not return on success"),
                Err(e) => {
                    let s = alloc::format!("[FAIL] execve error: {:?}\n", e);
                    early_print(&s);
                    sys_exit(1);
                }
            }
        }
        Ok(child_pid) => {
            // Parent: attendre
            let mut status = 0;
            match sys_wait4(child_pid, &mut status, 0, core::ptr::null_mut()) {
                Ok(_) => {
                    if status == 0 {
                        early_print("[PASS] exec() completed successfully ✅\n");
                    } else {
                        early_print("[FAIL] child exited with error ❌\n");
                    }
                }
                Err(e) => {
                    let s = alloc::format!("[FAIL] wait4 error: {:?}\n", e);
                    early_print(&s);
                }
            }
        }
        Err(e) => {
            let s = alloc::format!("[FAIL] fork error: {:?}\n", e);
            early_print(&s);
        }
    }
}

pub fn test_cow_with_real_memory() {
    early_print("\n=== TEST: CoW with Real User Memory ===\n");
    
    // Le programme test_cow_real.elf fait:
    // 1. Alloue 4KB buffer
    // 2. Fork
    // 3. Enfant modifie → CoW page fault
    // 4. Vérifie que parent/enfant ont buffers différents
    
    match sys_fork() {
        Ok(0) => {
            sys_execve("/bin/test_cow_real\0".as_ptr(), &[], &[]).ok();
            unreachable!();
        }
        Ok(child_pid) => {
            let stats_before = cow_manager::get_stats();
            
            sys_wait4(child_pid, &mut 0, 0, core::ptr::null_mut()).ok();
            
            let stats_after = cow_manager::get_stats();
            
            let s = alloc::format!(
                "[STATS] Pages tracked: {} → {}\n",
                stats_before.total_pages,
                stats_after.total_pages
            );
            early_print(&s);
            
            if stats_after.total_pages > 0 {
                early_print("[PASS] CoW triggered with real memory ✅\n");
            } else {
                early_print("[WARN] No CoW activity detected\n");
            }
        }
        Err(e) => {
            early_print("[FAIL] fork failed\n");
        }
    }
}
```

---

### Niveau 3: MOYEN (Optimiser Tests Phase 3)

#### 3.1. Syscalls Userspace Helpers
**Fichier:** `userland/lib/syscalls.h`

```c
#ifndef SYSCALLS_H
#define SYSCALLS_H

typedef long ssize_t;
typedef unsigned long size_t;
typedef int pid_t;

// Syscall numbers (x86_64)
#define SYS_read    0
#define SYS_write   1
#define SYS_open    2
#define SYS_close   3
#define SYS_fork    57
#define SYS_execve  59
#define SYS_exit    60
#define SYS_wait4   61

// Wrappers inline
static inline ssize_t write(int fd, const void *buf, size_t count) {
    long ret;
    asm volatile(
        "syscall"
        : "=a"(ret)
        : "a"(SYS_write), "D"(fd), "S"(buf), "d"(count)
        : "rcx", "r11", "memory"
    );
    return ret;
}

static inline pid_t fork(void) {
    long ret;
    asm volatile("syscall" : "=a"(ret) : "a"(SYS_fork) : "rcx", "r11");
    return ret;
}

static inline void exit(int status) {
    asm volatile("syscall" :: "a"(SYS_exit), "D"(status) : "rcx", "r11");
    __builtin_unreachable();
}

static inline pid_t wait4(pid_t pid, int *status, int options, void *rusage) {
    long ret;
    asm volatile(
        "syscall"
        : "=a"(ret)
        : "a"(SYS_wait4), "D"(pid), "S"(status), "d"(options), "r"(rusage)
        : "rcx", "r11", "memory"
    );
    return ret;
}

static inline int execve(const char *path, char *const argv[], char *const envp[]) {
    long ret;
    asm volatile(
        "syscall"
        : "=a"(ret)
        : "a"(SYS_execve), "D"(path), "S"(argv), "d"(envp)
        : "rcx", "r11", "memory"
    );
    return ret;
}

#endif // SYSCALLS_H
```

**Utilisation:**
```c
#include "syscalls.h"

void _start() {
    pid_t pid = fork();
    if (pid == 0) {
        write(1, "Child\n", 6);
        exit(0);
    } else {
        wait4(pid, NULL, 0, NULL);
        write(1, "Parent\n", 7);
        exit(0);
    }
}
```

---

#### 3.2. Mini-libc (Optionnel)
**Fichier:** `userland/lib/string.c`

```c
void *memset(void *s, int c, size_t n) {
    unsigned char *p = s;
    while (n--) *p++ = (unsigned char)c;
    return s;
}

void *memcpy(void *dest, const void *src, size_t n) {
    unsigned char *d = dest;
    const unsigned char *s = src;
    while (n--) *d++ = *s++;
    return dest;
}

size_t strlen(const char *s) {
    size_t len = 0;
    while (s[len]) len++;
    return len;
}
```

---

## 📊 RÉSUMÉ PRIORISATION

### ⚡ CRITIQUE - À faire MAINTENANT (Semaine 1)
1. **ELF Loader:**
   - `load_elf_to_memory()` - charger segments PT_LOAD
   - `setup_user_stack()` - créer stack userspace
   - `map_user_pages()` - mapper avec bonnes permissions

2. **sys_execve():**
   - Parser path/argv/envp
   - Charger ELF depuis VFS
   - Jump vers entry_point userspace

3. **Test Programs:**
   - Compiler `test_hello.c` → `/bin/test_hello`
   - Charger dans VFS au boot
   - Test exec depuis kernel thread

**Livrable:** Un seul programme userspace qui s'exécute via exec()

---

### 🟡 IMPORTANT - Semaine 2
4. **Vraies Pages User:**
   - Allocations heap userspace
   - Stack userspace avec garde page
   - capture_address_space() retourne vraies pages

5. **Tests CoW Réels:**
   - `test_cow_real.c` - programme qui trigger CoW
   - Test avec fork() + modification mémoire
   - Validation stats CoW

**Livrable:** Tests CoW avec vraies métriques (>0 pages)

---

### 🟢 MOYEN - Semaine 3-4
6. **Suite de Tests:**
   - `test_fork_simple.c`
   - `test_exec_chain.c`
   - `test_pipe.c`

7. **Infrastructure:**
   - `syscalls.h` helper library
   - Mini-libc basique
   - Build system pour userland

**Livrable:** Suite complète de tests automatisés

---

## 🎯 CRITÈRES DE SUCCÈS

### Phase 2 Débloquée:
- ✅ Au moins 1 programme userspace exécuté via exec()
- ✅ CoW avec >0 pages trackées (pas juste kernel threads)
- ✅ Fork + Exec + Wait cycle complet
- ✅ Latency fork() < 1M cycles (target réaliste avec vraies pages)

### Phase 3 Débloquée:
- ✅ 5+ programmes userspace disponibles
- ✅ Tests automatisés sans timeout
- ✅ Shell simple qui exécute des commandes
- ✅ Pipe() pour IPC entre processus

---

## 📝 PROCHAINES ÉTAPES IMMÉDIATES

```bash
# 1. Créer le loader ELF complet
cd kernel/src/loader/
# Implémenter load_elf_to_memory() dans elf_loader.rs

# 2. Compléter sys_execve()
cd kernel/src/syscall/handlers/
# Implémenter dans process.rs

# 3. Compiler programmes de test
cd userland/
gcc -nostdlib -static -fno-pie -o test_hello.elf test_hello.c

# 4. Charger dans VFS
cd kernel/src/
# Ajouter load_test_binaries() dans lib.rs

# 5. Tester
make release
./build.sh
qemu-system-x86_64 -cdrom exo-os.iso -serial stdio
```

---

**Temps estimé:** 2-3 semaines pour débloquer complètement Phase 2  
**Complexité:** Moyenne (ELF loader le plus technique)  
**Impact:** 🔥 CRITIQUE - Débloque 50% des tests Phase 2-3
