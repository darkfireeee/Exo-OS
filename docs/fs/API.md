# 🔌 API - Guide des APIs Filesystem Exo-OS

## 📋 Vue d'Ensemble

Ce document décrit toutes les APIs publiques du système de fichiers.

---

## 1. VFS APIs

### Trait Inode

```rust
pub trait Inode: Send + Sync {
    // Lecture à offset spécifique
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize>;
    
    // Écriture à offset spécifique
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> FsResult<usize>;
    
    // Lister contenu répertoire
    fn list(&self) -> FsResult<Vec<String>>;
    
    // Chercher entrée dans répertoire
    fn lookup(&self, name: &str) -> FsResult<u64>;
    
    // Créer nouveau fichier/répertoire
    fn create(&mut self, name: &str, inode_type: InodeType) -> FsResult<u64>;
    
    // Supprimer fichier/répertoire
    fn remove(&mut self, name: &str) -> FsResult<()>;
    
    // Tronquer fichier
    fn truncate(&mut self, size: u64) -> FsResult<()>;
}
```

### Trait FileOperations

```rust
pub trait FileOperations: Send + Sync {
    fn read(&self, buf: &mut [u8], offset: u64) -> FsResult<usize>;
    fn write(&mut self, buf: &[u8], offset: u64) -> FsResult<usize>;
    fn seek(&mut self, pos: SeekFrom) -> FsResult<u64>;
    fn ioctl(&mut self, request: u64, arg: usize) -> FsResult<i32>;
    fn sync(&mut self) -> FsResult<()>;
}
```

---

## 2. Filesystems Réels

### FAT32

```rust
// Créer filesystem FAT32
pub fn Fat32Fs::new(device: Arc<dyn BlockDevice>) -> FsResult<Self>;

// Monter partition
pub fn Fat32Fs::mount(device: Arc<dyn BlockDevice>) -> FsResult<Arc<Self>>;

// Ouvrir inode
pub fn Fat32Inode::open(fs: Arc<Fat32Fs>, cluster: u32) -> FsResult<Self>;
```

### ext4

```rust
// Créer filesystem ext4
pub fn Ext4Fs::new(device: Arc<dyn BlockDevice>) -> FsResult<Self>;

// Monter partition
pub fn Ext4Fs::mount(device: Arc<dyn BlockDevice>) -> FsResult<Arc<Self>>;

// Ouvrir inode
pub fn Ext4Inode::open(fs: Arc<Ext4Fs>, inode_num: u64) -> FsResult<Self>;
```

---

## 3. APIs POSIX

### I/O Basique

```c
// Ouvrir fichier
int open(const char *path, int flags, mode_t mode);

// Lire
ssize_t read(int fd, void *buf, size_t count);

// Écrire
ssize_t write(int fd, const void *buf, size_t count);

// Seek
off_t lseek(int fd, off_t offset, int whence);

// Fermer
int close(int fd);
```

### I/O Vectorisé

```c
// Lecture vectorisée
ssize_t readv(int fd, const struct iovec *iov, int iovcnt);

// Écriture vectorisée
ssize_t writev(int fd, const struct iovec *iov, int iovcnt);
```

### Async I/O (POSIX AIO)

```c
// Lecture asynchrone
int aio_read(struct aiocb *aiocbp);

// Écriture asynchrone
int aio_write(struct aiocb *aiocbp);

// Attendre complétion
int aio_suspend(const struct aiocb *const aiocb_list[], int nitems, 
                const struct timespec *timeout);

// Obtenir résultat
ssize_t aio_return(struct aiocb *aiocbp);

// Obtenir erreur
int aio_error(struct aiocb *aiocbp);
```

### io_uring

```c
// Setup io_uring
int io_uring_setup(unsigned entries, struct io_uring_params *p);

// Soumettre requêtes
int io_uring_enter(int fd, unsigned to_submit, unsigned min_complete,
                   unsigned flags, sigset_t *sig);

// Enregistrer buffers/fichiers
int io_uring_register(int fd, unsigned opcode, void *arg, unsigned nr_args);
```

### Zero-Copy

```c
// sendfile (file → socket)
ssize_t sendfile(int out_fd, int in_fd, off_t *offset, size_t count);

// splice (pipe ↔ file/socket)
ssize_t splice(int fd_in, off_t *off_in, int fd_out, off_t *off_out,
               size_t len, unsigned int flags);

// vmsplice (user buffer → pipe)
ssize_t vmsplice(int fd, const struct iovec *iov, unsigned long nr_segs,
                 unsigned int flags);

// tee (pipe → pipe, sans consommer)
ssize_t tee(int fd_in, int fd_out, size_t len, unsigned int flags);
```

### Memory Mapping

```c
// Mapper fichier en mémoire
void *mmap(void *addr, size_t length, int prot, int flags, 
           int fd, off_t offset);

// Dé-mapper
int munmap(void *addr, size_t length);

// Synchroniser avec disque
int msync(void *addr, size_t length, int flags);

// Advice au kernel
int madvise(void *addr, size_t length, int advice);

// Protection mémoire
int mprotect(void *addr, size_t len, int prot);
```

### File Locking

```c
// POSIX record locks
int fcntl(int fd, int cmd, struct flock *lock);
// cmd: F_SETLK, F_SETLKW, F_GETLK

// BSD flock
int flock(int fd, int operation);
// operation: LOCK_SH, LOCK_EX, LOCK_UN
```

### Disk Quotas

```c
// Contrôle quotas
int quotactl(int cmd, const char *special, int id, caddr_t addr);

// Commandes:
// Q_QUOTAON  - Activer quotas
// Q_QUOTAOFF - Désactiver quotas
// Q_GETQUOTA - Obtenir quota utilisateur
// Q_SETQUOTA - Définir quota utilisateur
```

### ACLs (Access Control Lists)

```c
// Obtenir ACL
acl_t acl_get_file(const char *path, acl_type_t type);

// Définir ACL
int acl_set_file(const char *path, acl_type_t type, acl_t acl);

// Obtenir entrée ACL
int acl_get_entry(acl_t acl, int entry_id, acl_entry_t *entry_p);

// Créer entrée ACL
int acl_create_entry(acl_t *acl_p, acl_entry_t *entry_p);

// Libérer ACL
int acl_free(void *obj_p);
```

### inotify (File Notifications)

```c
// Créer instance inotify
int inotify_init(void);
int inotify_init1(int flags);

// Ajouter watch
int inotify_add_watch(int fd, const char *pathname, uint32_t mask);

// Retirer watch
int inotify_rm_watch(int fd, int wd);

// Lire events
ssize_t read(int fd, void *buf, size_t count);
// Retourne struct inotify_event[]

// Events disponibles:
// IN_CREATE, IN_DELETE, IN_MODIFY, IN_MOVE, IN_ATTRIB,
// IN_OPEN, IN_CLOSE, IN_ACCESS, etc.
```

### Mount Namespaces

```c
// Créer namespace
int unshare(int flags);
// flags: CLONE_NEWNS

// Changer root filesystem
int pivot_root(const char *new_root, const char *put_old);

// Mount filesystem
int mount(const char *source, const char *target,
          const char *filesystemtype, unsigned long mountflags,
          const void *data);

// Unmount
int umount(const char *target);
int umount2(const char *target, int flags);
```

---

## 4. Exemples Complets

### Lecture Fichier

```rust
use exo_os::fs::{vfs, FsError};

fn read_file(path: &str) -> Result<Vec<u8>, FsError> {
    // Ouvrir via VFS
    let inode = vfs::open(path)?;
    
    // Lire contenu
    let mut buf = vec![0u8; 4096];
    let n = inode.read_at(0, &mut buf)?;
    buf.truncate(n);
    
    Ok(buf)
}
```

### Async I/O avec io_uring

```c
#include <liburing.h>

int async_read_file(const char *path) {
    struct io_uring ring;
    struct io_uring_sqe *sqe;
    struct io_uring_cqe *cqe;
    char buf[4096];
    int fd, ret;
    
    // Setup io_uring
    io_uring_queue_init(256, &ring, 0);
    
    // Ouvrir fichier
    fd = open(path, O_RDONLY);
    
    // Obtenir SQE
    sqe = io_uring_get_sqe(&ring);
    
    // Préparer read
    io_uring_prep_read(sqe, fd, buf, sizeof(buf), 0);
    
    // Soumettre
    io_uring_submit(&ring);
    
    // Attendre complétion
    io_uring_wait_cqe(&ring, &cqe);
    
    // Résultat
    ret = cqe->res;
    
    // Cleanup
    io_uring_cqe_seen(&ring, cqe);
    io_uring_queue_exit(&ring);
    close(fd);
    
    return ret;
}
```

### Zero-Copy Transfer

```c
#include <sys/sendfile.h>

// Copier fichier via sendfile (zero-copy)
int copy_file_zerocopy(const char *src, const char *dst) {
    int src_fd = open(src, O_RDONLY);
    int dst_fd = open(dst, O_WRONLY | O_CREAT, 0644);
    struct stat st;
    
    fstat(src_fd, &st);
    
    // sendfile = zero-copy kernel-space
    off_t offset = 0;
    ssize_t sent = sendfile(dst_fd, src_fd, &offset, st.st_size);
    
    close(src_fd);
    close(dst_fd);
    
    return sent == st.st_size ? 0 : -1;
}
```

### Memory Mapping

```c
#include <sys/mman.h>

// Mapper fichier en mémoire
void *map_file(const char *path, size_t *size) {
    int fd = open(path, O_RDONLY);
    struct stat st;
    fstat(fd, &st);
    *size = st.st_size;
    
    // Mapper en mémoire
    void *addr = mmap(NULL, *size, PROT_READ, MAP_SHARED, fd, 0);
    
    close(fd);
    return addr;
}

// Utilisation
size_t size;
char *data = map_file("/path/to/file", &size);

// Accès direct (pas de read() nécessaire)
printf("Premier char: %c\n", data[0]);

// Cleanup
munmap(data, size);
```

### inotify Monitoring

```c
#include <sys/inotify.h>

#define EVENT_SIZE (sizeof(struct inotify_event))
#define BUF_LEN (1024 * (EVENT_SIZE + 16))

int monitor_directory(const char *path) {
    int fd = inotify_init();
    int wd = inotify_add_watch(fd, path, 
        IN_CREATE | IN_DELETE | IN_MODIFY | IN_MOVED_FROM | IN_MOVED_TO);
    
    char buf[BUF_LEN];
    
    while (1) {
        int length = read(fd, buf, BUF_LEN);
        
        int i = 0;
        while (i < length) {
            struct inotify_event *event = (struct inotify_event *)&buf[i];
            
            if (event->mask & IN_CREATE) {
                printf("Fichier créé: %s\n", event->name);
            }
            else if (event->mask & IN_DELETE) {
                printf("Fichier supprimé: %s\n", event->name);
            }
            else if (event->mask & IN_MODIFY) {
                printf("Fichier modifié: %s\n", event->name);
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

## 5. Résumé API Reference

| API | Header | Syscall | Description |
|-----|--------|---------|-------------|
| `open()` | `<fcntl.h>` | 2 | Ouvrir fichier |
| `read()` | `<unistd.h>` | 0 | Lire |
| `write()` | `<unistd.h>` | 1 | Écrire |
| `close()` | `<unistd.h>` | 3 | Fermer |
| `lseek()` | `<unistd.h>` | 8 | Seek |
| `readv()` | `<sys/uio.h>` | 19 | Lecture vectorisée |
| `writev()` | `<sys/uio.h>` | 20 | Écriture vectorisée |
| `sendfile()` | `<sys/sendfile.h>` | 40 | Zero-copy transfer |
| `mmap()` | `<sys/mman.h>` | 9 | Memory mapping |
| `munmap()` | `<sys/mman.h>` | 11 | Unmap |
| `fcntl()` | `<fcntl.h>` | 72 | File control (locks) |
| `flock()` | `<sys/file.h>` | 73 | BSD file locking |
| `io_uring_setup()` | `<liburing.h>` | 425 | Setup io_uring |
| `io_uring_enter()` | `<liburing.h>` | 426 | Enter io_uring |
| `inotify_init()` | `<sys/inotify.h>` | 253 | Init inotify |
| `inotify_add_watch()` | `<sys/inotify.h>` | 254 | Add watch |
| `quotactl()` | `<sys/quota.h>` | 179 | Quota control |
| `acl_get_file()` | `<sys/acl.h>` | - | Get ACL |
| `pivot_root()` | `<unistd.h>` | 155 | Change root |

---

Pour plus de détails :
- [ARCHITECTURE.md](./ARCHITECTURE.md) : Architecture technique
- [PERFORMANCE.md](./PERFORMANCE.md) : Optimisations
- [EXAMPLES.md](./EXAMPLES.md) : Exemples pratiques complets
