# Inter-Process Communication (IPC) Guide

## Table of Contents

1. [Overview](#overview)
2. [Pipes & FIFOs](#pipes--fifos)
3. [Unix Domain Sockets](#unix-domain-sockets)
4. [Network Sockets](#network-sockets)
5. [System V IPC](#system-v-ipc)
6. [Event File Descriptors](#event-file-descriptors)
7. [Best Practices](#best-practices)

---

## Overview

POSIX-X implements comprehensive IPC facilities for inter-process and inter-thread communication. All mechanisms are integrated with the file descriptor table for uniform I/O operations.

**Supported Mechanisms**:

- **Pipes**: Anonymous and named (FIFOs)
- **Unix Domain Sockets**: Local socket communication
- **Network Sockets**: TCP/UDP (AF_INET, AF_INET6)
- **System V IPC**: Shared memory, semaphores, message queues
- **Event FDs**: eventfd, signalfd

---

## Pipes & FIFOs

### Anonymous Pipes

**Implementation**: `kernel/src/syscall/handlers/fs_fifo.rs`

#### Creating Pipes

```c
#include <unistd.h>

int main() {
    int pipefd[2];
    
    if (pipe(pipefd) == -1) {
        perror("pipe");
        return 1;
    }
    
    // pipefd[0] = read end
    // pipefd[1] = write end
    
    pid_t pid = fork();
    if (pid == 0) {
        // Child: read from pipe
        close(pipefd[1]);  // Close write end
        
        char buf[100];
        ssize_t n = read(pipefd[0], buf, sizeof(buf));
        write(STDOUT_FILENO, buf, n);
        
        close(pipefd[0]);
    } else {
        // Parent: write to pipe
        close(pipefd[0]);  // Close read end
        
        write(pipefd[1], "Hello from parent!\n", 19);
        
        close(pipefd[1]);
        wait(NULL);
    }
    
    return 0;
}
```

#### Pipe Implementation

```rust
pub unsafe fn sys_pipe(pipefd: *mut [i32; 2]) -> i64 {
    if pipefd.is_null() {
        return -14; // EFAULT
    }
    
    // Create pipe buffer
    let buffer = Arc::new(RwLock::new(PipeBuffer::new(65536)));
    
    // Create read handle
    let read_handle = VfsHandle {
        inode: create_pipe_inode(buffer.clone(), PipeEnd::Read),
        offset: AtomicU64::new(0),
        flags: OpenFlags::READABLE,
    };
    
    // Create write handle
    let write_handle = VfsHandle {
        inode: create_pipe_inode(buffer, PipeEnd::Write),
        offset: AtomicU64::new(0),
        flags: OpenFlags::WRITABLE,
    };
    
    // Add to FD table
    let read_fd = GLOBAL_FD_TABLE.insert(read_handle)?;
    let write_fd = GLOBAL_FD_TABLE.insert(write_handle)?;
    
    (*pipefd)[0] = read_fd;
    (*pipefd)[1] = write_fd;
    
    0
}
```

#### Pipe Buffer

```rust
struct PipeBuffer {
    data: Vec<u8>,
    read_pos: usize,
    write_pos: usize,
    capacity: usize,
    readers: AtomicUsize,
    writers: AtomicUsize,
}

impl PipeBuffer {
    fn read(&mut self, buf: *mut u8, len: usize) -> Result<usize> {
        // Block if empty and writers exist
        while self.is_empty() && self.writers.load(Ordering::SeqCst) > 0 {
            // Wait for data or writer close
        }
        
        // EOF if no writers
        if self.is_empty() && self.writers.load(Ordering::SeqCst) == 0 {
            return Ok(0);
        }
        
        // Copy available data
        let available = self.available();
        let to_read = core::cmp::min(len, available);
        
        for i in 0..to_read {
            unsafe { *buf.add(i) = self.data[self.read_pos]; }
            self.read_pos = (self.read_pos + 1) % self.capacity;
        }
        
        Ok(to_read)
    }
    
    fn write(&mut self, buf: *const u8, len: usize) -> Result<usize> {
        // SIGPIPE if no readers
        if self.readers.load(Ordering::SeqCst) == 0 {
            return Err(PipeError::BrokenPipe);
        }
        
        // Block if full
        while self.is_full() {
            // Wait for space
        }
        
        // Write available space
        let space = self.free_space();
        let to_write = core::cmp::min(len, space);
        
        for i in 0..to_write {
            unsafe {
                self.data[self.write_pos] = *buf.add(i);
            }
            self.write_pos = (self.write_pos + 1) % self.capacity;
        }
        
        Ok(to_write)
    }
}
```

### Named Pipes (FIFOs)

```c
#include <sys/stat.h>
#include <fcntl.h>

int main() {
    // Create FIFO
    mkfifo("/tmp/myfifo", 0666);
    
    pid_t pid = fork();
    if (pid == 0) {
        // Child: read from FIFO
        int fd = open("/tmp/myfifo", O_RDONLY);
        char buf[100];
        read(fd, buf, sizeof(buf));
        printf("Received: %s\n", buf);
        close(fd);
    } else {
        // Parent: write to FIFO
        int fd = open("/tmp/myfifo", O_WRONLY);
        write(fd, "Hello via FIFO!", 15);
        close(fd);
        wait(NULL);
    }
    
    unlink("/tmp/myfifo");
    return 0;
}
```

---

## Unix Domain Sockets

**Implementation**: `kernel/src/syscall/handlers/net_socket.rs`

### Stream Sockets (SOCK_STREAM)

```c
#include <sys/socket.h>
#include <sys/un.h>

// Server
int server() {
    int server_fd = socket(AF_UNIX, SOCK_STREAM, 0);
    
    struct sockaddr_un addr;
    addr.sun_family = AF_UNIX;
    strcpy(addr.sun_path, "/tmp/server.sock");
    
    unlink("/tmp/server.sock");  // Remove if exists
    bind(server_fd, (struct sockaddr*)&addr, sizeof(addr));
    listen(server_fd, 5);
    
    int client_fd = accept(server_fd, NULL, NULL);
    
    char buf[100];
    ssize_t n = read(client_fd, buf, sizeof(buf));
    write(STDOUT_FILENO, buf, n);
    
    close(client_fd);
    close(server_fd);
    return 0;
}

// Client
int client() {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    
    struct sockaddr_un addr;
    addr.sun_family = AF_UNIX;
    strcpy(addr.sun_path, "/tmp/server.sock");
    
    connect(fd, (struct sockaddr*)&addr, sizeof(addr));
    write(fd, "Hello server!", 13);
    
    close(fd);
    return 0;
}
```

### Datagram Sockets (SOCK_DGRAM)

```c
int server() {
    int fd = socket(AF_UNIX, SOCK_DGRAM, 0);
    
    struct sockaddr_un addr;
    addr.sun_family = AF_UNIX;
    strcpy(addr.sun_path, "/tmp/dgram.sock");
    
    unlink("/tmp/dgram.sock");
    bind(fd, (struct sockaddr*)&addr, sizeof(addr));
    
    char buf[100];
    struct sockaddr_un client_addr;
    socklen_t len = sizeof(client_addr);
    
    ssize_t n = recvfrom(fd, buf, sizeof(buf), 0,
                         (struct sockaddr*)&client_addr, &len);
    
    printf("Received: %.*s\n", (int)n, buf);
    
    close(fd);
    return 0;
}
```

### Socket Pairs

```c
#include <sys/socket.h>

int main() {
    int sv[2];
    
    // Create connected socket pair
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == -1) {
        perror("socketpair");
        return 1;
    }
    
    // sv[0] and sv[1] are connected
    // Like a bidirectional pipe
    
    if (fork() == 0) {
        // Child
        close(sv[1]);
        char buf[100];
        read(sv[0], buf, sizeof(buf));
        printf("Child received: %s\n", buf);
        write(sv[0], "Response from child", 19);
        close(sv[0]);
    } else {
        // Parent
        close(sv[0]);
        write(sv[1], "Message to child", 16);
        char buf[100];
        read(sv[1], buf, sizeof(buf));
        printf("Parent received: %s\n", buf);
        close(sv[1]);
        wait(NULL);
    }
    
    return 0;
}
```

### Implementation Details

```rust
pub unsafe fn sys_socketpair(
    domain: i32,
    type_: i32,
    protocol: i32,
    sv: *mut [i32; 2]
) -> i64 {
    if domain != AF_UNIX {
        return -97; // EAFNOSUPPORT
    }
    
    // Create shared buffer
    let buffer = Arc::new(RwLock::new(SocketBuffer::new()));
    
    // Create socket handles
    let sock1 = create_unix_socket(buffer.clone(), SocketEnd::First);
    let sock2 = create_unix_socket(buffer.clone(), SocketEnd::Second);
    
    // Add to FD table
    let fd1 = GLOBAL_FD_TABLE.insert(sock1)?;
    let fd2 = GLOBAL_FD_TABLE.insert(sock2)?;
    
    (*sv)[0] = fd1;
    (*sv)[1] = fd2;
    
    0
}
```

---

## Network Sockets

### TCP Server

```c
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>

int main() {
    int server_fd = socket(AF_INET, SOCK_STREAM, 0);
    
    int opt = 1;
    setsockopt(server_fd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));
    
    struct sockaddr_in addr;
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = INADDR_ANY;
    addr.sin_port = htons(8080);
    
    bind(server_fd, (struct sockaddr*)&addr, sizeof(addr));
    listen(server_fd, 10);
    
    printf("Server listening on port 8080\n");
    
    while (1) {
        struct sockaddr_in client_addr;
        socklen_t len = sizeof(client_addr);
        
        int client_fd = accept(server_fd, (struct sockaddr*)&client_addr, &len);
        
        char buf[1024];
        ssize_t n = read(client_fd, buf, sizeof(buf) - 1);
        buf[n] = '\0';
        
        printf("Received: %s\n", buf);
        
        write(client_fd, "HTTP/1.1 200 OK\r\n\r\nHello!\n", 27);
        close(client_fd);
    }
    
    return 0;
}
```

### TCP Client

```c
int main() {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    
    struct sockaddr_in addr;
    addr.sin_family = AF_INET;
    addr.sin_port = htons(8080);
    inet_pton(AF_INET, "127.0.0.1", &addr.sin_addr);
    
    if (connect(fd, (struct sockaddr*)&addr, sizeof(addr)) == -1) {
        perror("connect");
        return 1;
    }
    
    write(fd, "GET / HTTP/1.1\r\n\r\n", 18);
    
    char buf[1024];
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    buf[n] = '\0';
    printf("Response: %s\n", buf);
    
    close(fd);
    return 0;
}
```

### UDP Sockets

```c
// UDP Server
int udp_server() {
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    
    struct sockaddr_in addr;
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = INADDR_ANY;
    addr.sin_port = htons(5000);
    
    bind(fd, (struct sockaddr*)&addr, sizeof(addr));
    
    char buf[1024];
    struct sockaddr_in client_addr;
    socklen_t len = sizeof(client_addr);
    
    ssize_t n = recvfrom(fd, buf, sizeof(buf), 0,
                         (struct sockaddr*)&client_addr, &len);
    
    printf("Received %zd bytes\n", n);
    
    sendto(fd, "ACK", 3, 0, (struct sockaddr*)&client_addr, len);
    
    close(fd);
    return 0;
}
```

---

## System V IPC

### Shared Memory

**Implementation**: `kernel/src/syscall/handlers/ipc_sysv.rs`

```c
#include <sys/shm.h>
#include <sys/ipc.h>

int main() {
    key_t key = ftok("/tmp/shmkey", 'R');
    
    // Create shared memory segment (1MB)
    int shmid = shmget(key, 1024 * 1024, IPC_CREAT | 0666);
    if (shmid < 0) {
        perror("shmget");
        return 1;
    }
    
    // Attach to address space
    void *ptr = shmat(shmid, NULL, 0);
    if (ptr == (void*)-1) {
        perror("shmat");
        return 1;
    }
    
    // Write to shared memory
    sprintf((char*)ptr, "Hello from process %d", getpid());
    
    // Detach
    shmdt(ptr);
    
    // In another process:
    // void *ptr = shmat(shmid, NULL, 0);
    // printf("Read: %s\n", (char*)ptr);
    // shmdt(ptr);
    
    // Cleanup
    shmctl(shmid, IPC_RMID, NULL);
    
    return 0;
}
```

#### Implementation

```rust
static SHARED_MEMORY: Mutex<BTreeMap<i32, Arc<RwLock<Vec<u8>>>>> = 
    Mutex::new(BTreeMap::new());
static NEXT_SHM_ID: AtomicI32 = AtomicI32::new(1);

pub unsafe fn sys_shmget(key: i32, size: usize, shmflg: i32) -> i64 {
    if size == 0 || size > MAX_SHM_SIZE {
        return -22; // EINVAL
    }
    
    let mut shm_table = SHARED_MEMORY.lock();
    
    // Check if key exists
    if key != IPC_PRIVATE {
        for (id, segment) in shm_table.iter() {
            if segment_key(*id) == key {
                if shmflg & IPC_CREAT != 0 && shmflg & IPC_EXCL != 0 {
                    return -17; // EEXIST
                }
                return *id as i64;
            }
        }
    }
    
    // Create new segment
    let shmid = NEXT_SHM_ID.fetch_add(1, Ordering::SeqCst);
    let segment = Arc::new(RwLock::new(vec![0u8; size]));
    shm_table.insert(shmid, segment);
    
    shmid as i64
}

pub unsafe fn sys_shmat(shmid: i32, shmaddr: *const u8, shmflg: i32) -> i64 {
    let shm_table = SHARED_MEMORY.lock();
    
    let segment = shm_table.get(&shmid)
        .ok_or(-22)?; // EINVAL
    
    // Map into address space
    let addr = if shmaddr.is_null() {
        // Kernel chooses address
        allocate_shm_mapping(segment.clone())?
    } else {
        // User specifies address
        shmaddr as u64
    };
    
    addr as i64
}
```

### Semaphores

```c
#include <sys/sem.h>

int main() {
    key_t key = ftok("/tmp/semkey", 'S');
    
    // Create semaphore set with 1 semaphore
    int semid = semget(key, 1, IPC_CREAT | 0666);
    
    // Initialize to 1 (binary semaphore/mutex)
    semctl(semid, 0, SETVAL, 1);
    
    // Lock (P operation)
    struct sembuf op_lock = {0, -1, 0};
    semop(semid, &op_lock, 1);
    
    // Critical section
    printf("In critical section\n");
    sleep(1);
    
    // Unlock (V operation)
    struct sembuf op_unlock = {0, 1, 0};
    semop(semid, &op_unlock, 1);
    
    // Cleanup
    semctl(semid, 0, IPC_RMID);
    
    return 0;
}
```

### Message Queues

```c
#include <sys/msg.h>

struct message {
    long mtype;
    char mtext[100];
};

int main() {
    key_t key = ftok("/tmp/msgkey", 'Q');
    
    // Create message queue
    int msgid = msgget(key, IPC_CREAT | 0666);
    
    if (fork() == 0) {
        // Child: receive messages
        struct message msg;
        msgrcv(msgid, &msg, sizeof(msg.mtext), 1, 0);
        printf("Received: %s\n", msg.mtext);
    } else {
        // Parent: send message
        struct message msg;
        msg.mtype = 1;
        strcpy(msg.mtext, "Hello via message queue!");
        msgsnd(msgid, &msg, strlen(msg.mtext) + 1, 0);
        
        wait(NULL);
        msgctl(msgid, IPC_RMID, NULL);
    }
    
    return 0;
}
```

---

## Event File Descriptors

### eventfd

```c
#include <sys/eventfd.h>

int main() {
    // Create eventfd with initial value 0
    int efd = eventfd(0, EFD_NONBLOCK);
    
    if (fork() == 0) {
        // Child: wait for event
        uint64_t val;
        read(efd, &val, sizeof(val));
        printf("Event received: %llu\n", val);
        close(efd);
    } else {
        // Parent: signal event
        sleep(1);
        uint64_t val = 42;
        write(efd, &val, sizeof(val));
        
        wait(NULL);
        close(efd);
    }
    
    return 0;
}
```

#### eventfd Implementation

```rust
pub unsafe fn sys_eventfd(initval: u32, flags: i32) -> i64 {
    let counter = Arc::new(AtomicU64::new(initval as u64));
    let semaphore = flags & EFD_SEMAPHORE != 0;
    
    let handle = VfsHandle {
        inode: create_eventfd_inode(counter, semaphore),
        offset: AtomicU64::new(0),
        flags: OpenFlags::READABLE | OpenFlags::WRITABLE,
    };
    
    let fd = GLOBAL_FD_TABLE.insert(handle)?;
    Ok(fd as i64)
}

// Read decrements counter
fn eventfd_read(counter: &AtomicU64, semaphore: bool) -> Result<u64> {
    loop {
        let current = counter.load(Ordering::SeqCst);
        if current == 0 {
            // Block if non-blocking not set
            continue;
        }
        
        let new_val = if semaphore {
            current - 1  // Decrement by 1
        } else {
            0  // Reset to 0
        };
        
        if counter.compare_exchange(current, new_val, 
                                    Ordering::SeqCst,
                                    Ordering::SeqCst).is_ok() {
            return Ok(current);
        }
    }
}
```

### signalfd

```c
#include <sys/signalfd.h>
#include <signal.h>

int main() {
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGINT);
    sigaddset(&mask, SIGTERM);
    
    // Block signals normally
    sigprocmask(SIG_BLOCK, &mask, NULL);
    
    // Create signalfd
    int sfd = signalfd(-1, &mask, 0);
    
    printf("Waiting for signals (try Ctrl+C)\n");
    
    struct signalfd_siginfo fdsi;
    ssize_t s = read(sfd, &fdsi, sizeof(fdsi));
    
    if (s == sizeof(fdsi)) {
        printf("Received signal %d\n", fdsi.ssi_signo);
    }
    
    close(sfd);
    return 0;
}
```

---

## Best Practices

### Choosing the Right IPC Mechanism

| Use Case | Recommended Mechanism | Why |
|----------|----------------------|-----|
| Parent-child communication | Pipe | Simple, efficient |
| Bidirectional parent-child | Socket pair | Full duplex |
| Local client-server | Unix domain socket | Fast, reliable |
| Network communication | TCP/UDP socket | Network support |
| Large data sharing | Shared memory | Zero-copy |
| Synchronization | Semaphore | Atomic operations |
| Message passing | Message queue | Structured messages |
| Event notification | eventfd | Lightweight |
| Signal reading | signalfd | Unified with select/poll/epoll |

### Error Handling

```c
// Always check return values
int fd = socket(AF_UNIX, SOCK_STREAM, 0);
if (fd < 0) {
    perror("socket");
    return 1;
}

// Close in error paths
if (bind(fd, ...) < 0) {
    perror("bind");
    close(fd);  // Don't leak FD
    return 1;
}
```

### Resource Cleanup

```c
// Pipes: close both ends when done
close(pipefd[0]);
close(pipefd[1]);

// Unix sockets: unlink socket file
close(sockfd);
unlink("/tmp/mysock");

// Shared memory: detach and remove
shmdt(ptr);
shmctl(shmid, IPC_RMID, NULL);

// Semaphores: remove when done
semctl(semid, 0, IPC_RMID);

// Message queues: remove when done
msgctl(msgid, IPC_RMID, NULL);
```

### Avoiding Deadlocks

```c
// Bad: Can deadlock if both processes do this
write(pipe1[1], data, size);  // Blocks if pipe1 full
read(pipe2[0], data, size);   // Never reached

// Good: Use non-blocking or select/poll
fcntl(pipe1[1], F_SETFL, O_NONBLOCK);
ssize_t n = write(pipe1[1], data, size);
if (n < 0 && errno == EAGAIN) {
    // Would block, try later
}
```

### Performance Tips

```c
// Use larger buffers for bulk transfers
char buf[65536];  // Better than 1024 for large files

// Use sendfile() for zero-copy file transfers
sendfile(out_fd, in_fd, NULL, file_size);

// Use splice() for pipe-to-pipe transfers
splice(in_fd, NULL, pipe_fd, NULL, size, 0);

// Use vectored I/O for scattered data
struct iovec iov[3];
// ... setup iov
writev(fd, iov, 3);
```

---

## Summary

POSIX-X provides complete IPC support:

- ✅ **Pipes**: Anonymous and named (FIFOs)
- ✅ **Unix Sockets**: Stream and datagram
- ✅ **Network Sockets**: TCP, UDP, IPv4/IPv6
- ✅ **System V IPC**: Shared memory, semaphores, message queues
- ✅ **Event FDs**: eventfd, signalfd
- ✅ **Zero-Copy**: sendfile, splice, tee

All mechanisms integrate with file descriptors for uniform select/poll/epoll support.

**Related Documentation**:

- [SYSCALL_REFERENCE.md](SYSCALL_REFERENCE.md) - Syscall details
- [ARCHITECTURE.md](ARCHITECTURE.md) - System overview
- [NETWORK_GUIDE.md](NETWORK_GUIDE.md) - Network details
