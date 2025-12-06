# 💡 EXAMPLES - Exemples Pratiques Filesystem

## 📋 Table des Matières

1. [I/O Basique](#io-basique)
2. [I/O Asynchrone](#io-asynchrone)
3. [Zero-Copy I/O](#zero-copy-io)
4. [Memory Mapping](#memory-mapping)
5. [File Locking](#file-locking)
6. [Disk Quotas](#disk-quotas)
7. [ACLs](#acls)
8. [inotify Monitoring](#inotify-monitoring)
9. [Containers & Namespaces](#containers--namespaces)

---

## I/O Basique

### Lire un Fichier Complet

```c
#include <fcntl.h>
#include <unistd.h>
#include <stdio.h>

int main() {
    int fd = open("/etc/passwd", O_RDONLY);
    if (fd < 0) {
        perror("open");
        return 1;
    }
    
    char buf[4096];
    ssize_t n;
    
    while ((n = read(fd, buf, sizeof(buf))) > 0) {
        write(STDOUT_FILENO, buf, n);
    }
    
    close(fd);
    return 0;
}
```

### Écrire dans un Fichier

```c
#include <fcntl.h>
#include <unistd.h>
#include <string.h>

int main() {
    int fd = open("/tmp/output.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        perror("open");
        return 1;
    }
    
    const char *data = "Hello, Exo-OS Filesystem!\n";
    write(fd, data, strlen(data));
    
    // Important: sync to disk
    fsync(fd);
    
    close(fd);
    return 0;
}
```

### Copie de Fichier Simple

```c
#include <fcntl.h>
#include <unistd.h>

int copy_file(const char *src, const char *dst) {
    int src_fd = open(src, O_RDONLY);
    int dst_fd = open(dst, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    
    if (src_fd < 0 || dst_fd < 0) {
        return -1;
    }
    
    char buf[4096];
    ssize_t n;
    
    while ((n = read(src_fd, buf, sizeof(buf))) > 0) {
        write(dst_fd, buf, n);
    }
    
    close(src_fd);
    close(dst_fd);
    
    return 0;
}
```

---

## I/O Asynchrone

### POSIX AIO - Lecture Asynchrone

```c
#include <aio.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>

int main() {
    int fd = open("/tmp/large_file.dat", O_RDONLY);
    
    // Préparer aiocb
    struct aiocb cb;
    memset(&cb, 0, sizeof(cb));
    
    char buf[4096];
    cb.aio_fildes = fd;
    cb.aio_buf = buf;
    cb.aio_nbytes = sizeof(buf);
    cb.aio_offset = 0;
    
    // Lancer lecture async
    aio_read(&cb);
    
    // Faire autre chose...
    printf("Reading asynchronously...\n");
    
    // Attendre complétion
    while (aio_error(&cb) == EINPROGRESS) {
        // Busy wait ou faire autre chose
    }
    
    // Obtenir résultat
    ssize_t n = aio_return(&cb);
    printf("Read %zd bytes\n", n);
    
    close(fd);
    return 0;
}
```

### io_uring - Multiple Ops Batch

```c
#include <liburing.h>
#include <fcntl.h>
#include <stdio.h>

int main() {
    struct io_uring ring;
    struct io_uring_sqe *sqe;
    struct io_uring_cqe *cqe;
    
    // Setup ring (256 entries)
    io_uring_queue_init(256, &ring, 0);
    
    // Ouvrir 10 fichiers
    int fds[10];
    char bufs[10][4096];
    
    for (int i = 0; i < 10; i++) {
        char path[64];
        sprintf(path, "/tmp/file%d.txt", i);
        fds[i] = open(path, O_RDONLY);
        
        // Préparer SQE
        sqe = io_uring_get_sqe(&ring);
        io_uring_prep_read(sqe, fds[i], bufs[i], sizeof(bufs[i]), 0);
        sqe->user_data = i;  // Tag pour identifier
    }
    
    // Soumettre tout en 1 syscall !
    io_uring_submit(&ring);
    
    // Attendre toutes complétions
    for (int i = 0; i < 10; i++) {
        io_uring_wait_cqe(&ring, &cqe);
        
        printf("File %llu: read %d bytes\n", cqe->user_data, cqe->res);
        
        io_uring_cqe_seen(&ring, cqe);
    }
    
    // Cleanup
    for (int i = 0; i < 10; i++) close(fds[i]);
    io_uring_queue_exit(&ring);
    
    return 0;
}
```

---

## Zero-Copy I/O

### sendfile - Copie Zero-Copy

```c
#include <sys/sendfile.h>
#include <fcntl.h>
#include <sys/stat.h>

// Copier fichier → socket (web server use-case)
int send_file_to_socket(int socket_fd, const char *file_path) {
    int file_fd = open(file_path, O_RDONLY);
    if (file_fd < 0) return -1;
    
    struct stat st;
    fstat(file_fd, &st);
    
    // sendfile = zero-copy kernel → network
    off_t offset = 0;
    ssize_t sent = sendfile(socket_fd, file_fd, &offset, st.st_size);
    
    close(file_fd);
    
    return sent == st.st_size ? 0 : -1;
}

// Copier fichier → fichier (zero-copy via pipe)
int copy_file_zerocopy(const char *src, const char *dst) {
    int src_fd = open(src, O_RDONLY);
    int dst_fd = open(dst, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    
    struct stat st;
    fstat(src_fd, &st);
    
    // Créer pipe intermédiaire
    int pipefd[2];
    pipe(pipefd);
    
    size_t total = 0;
    while (total < st.st_size) {
        // src → pipe
        ssize_t n = splice(src_fd, NULL, pipefd[1], NULL, 
                          65536, SPLICE_F_MOVE);
        if (n <= 0) break;
        
        // pipe → dst
        splice(pipefd[0], NULL, dst_fd, NULL, n, SPLICE_F_MOVE);
        total += n;
    }
    
    close(pipefd[0]);
    close(pipefd[1]);
    close(src_fd);
    close(dst_fd);
    
    return 0;
}
```

### vmsplice - User Buffer → Pipe

```c
#include <fcntl.h>
#include <sys/uio.h>

// Écrire buffer utilisateur dans pipe sans copie
int write_to_pipe_zerocopy(int pipe_fd, const void *data, size_t len) {
    struct iovec iov = {
        .iov_base = (void *)data,
        .iov_len = len,
    };
    
    // vmsplice = zero-copy user → kernel
    ssize_t n = vmsplice(pipe_fd, &iov, 1, SPLICE_F_GIFT);
    
    return n == len ? 0 : -1;
}
```

---

## Memory Mapping

### Mapper Fichier en Lecture

```c
#include <sys/mman.h>
#include <fcntl.h>
#include <stdio.h>

int main() {
    int fd = open("/tmp/data.bin", O_RDONLY);
    struct stat st;
    fstat(fd, &st);
    
    // Mapper fichier en mémoire
    void *addr = mmap(NULL, st.st_size, PROT_READ, MAP_SHARED, fd, 0);
    if (addr == MAP_FAILED) {
        perror("mmap");
        return 1;
    }
    
    // Accès direct (pas de read() !)
    unsigned char *data = (unsigned char *)addr;
    printf("Premier byte: 0x%02x\n", data[0]);
    
    // Cleanup
    munmap(addr, st.st_size);
    close(fd);
    
    return 0;
}
```

### Mapper Fichier en Écriture (Copy-on-Write)

```c
#include <sys/mman.h>
#include <fcntl.h>
#include <string.h>

int main() {
    int fd = open("/tmp/data.bin", O_RDWR);
    struct stat st;
    fstat(fd, &st);
    
    // MAP_PRIVATE = copy-on-write
    void *addr = mmap(NULL, st.st_size, PROT_READ | PROT_WRITE, 
                      MAP_PRIVATE, fd, 0);
    
    // Modifier (crée copie privée)
    char *data = (char *)addr;
    strcpy(data, "Modified!");
    
    // Fichier original inchangé (MAP_PRIVATE)
    
    munmap(addr, st.st_size);
    close(fd);
    
    return 0;
}
```

### Shared Memory via mmap

```c
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>

// Process 1: Créer shared memory
void process1() {
    int fd = open("/tmp/shm", O_RDWR | O_CREAT, 0666);
    ftruncate(fd, 4096);
    
    void *addr = mmap(NULL, 4096, PROT_READ | PROT_WRITE, 
                      MAP_SHARED, fd, 0);
    
    // Écrire
    strcpy((char *)addr, "Hello from process 1!");
    
    // Synchroniser
    msync(addr, 4096, MS_SYNC);
    
    sleep(10);  // Attendre process 2
    
    munmap(addr, 4096);
    close(fd);
}

// Process 2: Lire shared memory
void process2() {
    int fd = open("/tmp/shm", O_RDONLY);
    
    void *addr = mmap(NULL, 4096, PROT_READ, MAP_SHARED, fd, 0);
    
    // Lire
    printf("Message: %s\n", (char *)addr);
    
    munmap(addr, 4096);
    close(fd);
}
```

---

## File Locking

### POSIX Record Locks

```c
#include <fcntl.h>
#include <unistd.h>

// Locker portion de fichier
int lock_file_range(int fd, off_t start, off_t len) {
    struct flock fl = {
        .l_type = F_WRLCK,   // Write lock (exclusive)
        .l_whence = SEEK_SET,
        .l_start = start,
        .l_len = len,
    };
    
    // F_SETLKW = attendre si déjà locké
    return fcntl(fd, F_SETLKW, &fl);
}

// Unlocker
int unlock_file_range(int fd, off_t start, off_t len) {
    struct flock fl = {
        .l_type = F_UNLCK,
        .l_whence = SEEK_SET,
        .l_start = start,
        .l_len = len,
    };
    
    return fcntl(fd, F_SETLK, &fl);
}

// Exemple: Database avec row-level locking
int main() {
    int fd = open("/tmp/database.db", O_RDWR);
    
    // Locker row 42 (1KB par row)
    lock_file_range(fd, 42 * 1024, 1024);
    
    // Lire/modifier row
    char row[1024];
    pread(fd, row, sizeof(row), 42 * 1024);
    
    // ... modifications ...
    
    pwrite(fd, row, sizeof(row), 42 * 1024);
    
    // Unlocker
    unlock_file_range(fd, 42 * 1024, 1024);
    
    close(fd);
    return 0;
}
```

### BSD flock - Whole File Lock

```c
#include <sys/file.h>

int main() {
    int fd = open("/tmp/config.txt", O_RDWR);
    
    // Locker fichier complet (exclusive)
    flock(fd, LOCK_EX);
    
    // Modifier fichier
    write(fd, "new config\n", 11);
    
    // Unlocker
    flock(fd, LOCK_UN);
    
    close(fd);
    return 0;
}
```

---

## Disk Quotas

### Définir Quota Utilisateur

```c
#include <sys/quota.h>

int set_user_quota(const char *device, uid_t uid, 
                   uint64_t soft_blocks, uint64_t hard_blocks) {
    struct dqblk quota = {
        .dqb_bsoftlimit = soft_blocks,
        .dqb_bhardlimit = hard_blocks,
        .dqb_isoftlimit = 10000,  // 10k inodes soft
        .dqb_ihardlimit = 20000,  // 20k inodes hard
    };
    
    return quotactl(Q_SETQUOTA, device, uid, (caddr_t)&quota);
}

// Exemple
int main() {
    // User 1000: soft 1GB, hard 2GB
    set_user_quota("/dev/sda1", 1000, 
                   1024*1024*1024 / 512,  // blocks de 512B
                   2*1024*1024*1024 / 512);
    
    return 0;
}
```

### Obtenir Usage Quota

```c
#include <sys/quota.h>
#include <stdio.h>

void print_user_quota(const char *device, uid_t uid) {
    struct dqblk quota;
    
    if (quotactl(Q_GETQUOTA, device, uid, (caddr_t)&quota) < 0) {
        perror("quotactl");
        return;
    }
    
    printf("User %d quota:\n", uid);
    printf("  Blocks: %llu / %llu (soft) / %llu (hard)\n",
           quota.dqb_curspace / 512,
           quota.dqb_bsoftlimit,
           quota.dqb_bhardlimit);
    printf("  Inodes: %llu / %llu (soft) / %llu (hard)\n",
           quota.dqb_curinodes,
           quota.dqb_isoftlimit,
           quota.dqb_ihardlimit);
}
```

---

## ACLs

### Définir ACL sur Fichier

```c
#include <sys/acl.h>

int set_file_acl(const char *path) {
    acl_t acl = acl_init(5);
    
    // Owner: rwx
    acl_entry_t entry;
    acl_create_entry(&acl, &entry);
    acl_set_tag_type(entry, ACL_USER_OBJ);
    acl_permset_t perm;
    acl_get_permset(entry, &perm);
    acl_add_perm(perm, ACL_READ | ACL_WRITE | ACL_EXECUTE);
    
    // User alice: rw-
    acl_create_entry(&acl, &entry);
    acl_set_tag_type(entry, ACL_USER);
    acl_set_qualifier(entry, &(uid_t){1001});  // alice's UID
    acl_get_permset(entry, &perm);
    acl_add_perm(perm, ACL_READ | ACL_WRITE);
    
    // Group developers: r-x
    acl_create_entry(&acl, &entry);
    acl_set_tag_type(entry, ACL_GROUP);
    acl_set_qualifier(entry, &(gid_t){1000});  // developers GID
    acl_get_permset(entry, &perm);
    acl_add_perm(perm, ACL_READ | ACL_EXECUTE);
    
    // Appliquer
    acl_set_file(path, ACL_TYPE_ACCESS, acl);
    
    acl_free(acl);
    return 0;
}
```

---

## inotify Monitoring

### Surveiller Répertoire

```c
#include <sys/inotify.h>
#include <unistd.h>
#include <stdio.h>

#define EVENT_SIZE (sizeof(struct inotify_event))
#define BUF_LEN (1024 * (EVENT_SIZE + 16))

int main() {
    int fd = inotify_init();
    
    // Surveiller /tmp
    int wd = inotify_add_watch(fd, "/tmp",
        IN_CREATE | IN_DELETE | IN_MODIFY | IN_MOVED_FROM | IN_MOVED_TO);
    
    printf("Monitoring /tmp...\n");
    
    char buf[BUF_LEN];
    while (1) {
        int length = read(fd, buf, BUF_LEN);
        
        int i = 0;
        while (i < length) {
            struct inotify_event *event = (struct inotify_event *)&buf[i];
            
            if (event->len) {
                if (event->mask & IN_CREATE) {
                    printf("[CREATE] %s\n", event->name);
                }
                if (event->mask & IN_DELETE) {
                    printf("[DELETE] %s\n", event->name);
                }
                if (event->mask & IN_MODIFY) {
                    printf("[MODIFY] %s\n", event->name);
                }
                if (event->mask & IN_MOVED_FROM) {
                    printf("[MOVED_FROM] %s\n", event->name);
                }
                if (event->mask & IN_MOVED_TO) {
                    printf("[MOVED_TO] %s\n", event->name);
                }
            }
            
            i += EVENT_SIZE + event->len;
        }
    }
    
    inotify_rm_watch(fd, wd);
    close(fd);
    
    return 0;
}
```

---

## Containers & Namespaces

### Créer Container avec Mount Namespace

```c
#define _GNU_SOURCE
#include <sched.h>
#include <unistd.h>
#include <sys/mount.h>

// Container init process
int container_main(void *arg) {
    // Nouveau mount namespace créé par clone()
    
    // 1. Monter nouveau root
    mount("/path/to/rootfs", "/path/to/rootfs", NULL, MS_BIND, NULL);
    
    // 2. Changer root
    chdir("/path/to/rootfs");
    pivot_root(".", ".");
    umount2(".", MNT_DETACH);
    chdir("/");
    
    // 3. Monter pseudo-fs
    mount("proc", "/proc", "proc", 0, NULL);
    mount("tmpfs", "/tmp", "tmpfs", 0, NULL);
    
    // 4. Exec command
    execl("/bin/sh", "/bin/sh", NULL);
    
    return 0;
}

int main() {
    // Stack pour container
    char stack[4096];
    
    // Créer container avec nouveau mount namespace
    pid_t pid = clone(container_main, stack + sizeof(stack),
                      CLONE_NEWNS | SIGCHLD, NULL);
    
    // Attendre
    waitpid(pid, NULL, 0);
    
    return 0;
}
```

---

Pour plus de détails :
- [API.md](./API.md) : APIs complètes
- [ARCHITECTURE.md](./ARCHITECTURE.md) : Design technique
- [PERFORMANCE.md](./PERFORMANCE.md) : Optimisations
- [INTEGRATION.md](./INTEGRATION.md) : Guide intégration
