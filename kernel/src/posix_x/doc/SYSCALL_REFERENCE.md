# POSIX-X Syscall Reference

**Total Syscalls Implemented**: 127 / 140 (~91% POSIX compliance)

## Table of Contents

1. [Process Management](#process-management) (7 syscalls)
2. [Process Scheduling](#process-scheduling) (7 syscalls)
3. [Memory Management](#memory-management) (8 syscalls)
4. [File I/O](#file-io) (11 syscalls)
5. [Filesystem Operations](#filesystem-operations) (15 syscalls)
6. [File Metadata](#file-metadata) (9 syscalls)
7. [File Operations Advanced](#file-operations-advanced) (8 syscalls)
8. [Pipes & FIFOs](#pipes--fifos) (3 syscalls)
9. [Signals](#signals) (8 syscalls)
10. [Polling & Events](#polling--events) (4 syscalls)
11. [Sockets & Networking](#sockets--networking) (12 syscalls)
12. [System V IPC](#system-v-ipc) (11 syscalls)
13. [Event File Descriptors](#event-file-descriptors) (2 syscalls)
14. [Process Limits](#process-limits) (4 syscalls)
15. [File Notifications](#file-notifications) (4 syscalls)
16. [System Information](#system-information) (4 syscalls)
17. [Security & Capabilities](#security--capabilities) (10 syscalls)

---

## Process Management

### `fork()` - Create a child process

**Syscall Number**: 57

**Signature**:

```c
pid_t fork(void);
```

**Description**: Creates a new process by duplicating the calling process.

**Returns**:

- Child PID in parent process
- 0 in child process
- -1 on error (errno set)

**Example**:

```c
pid_t pid = fork();
if (pid == 0) {
    // Child process
    printf("I am the child!\n");
} else if (pid > 0) {
    // Parent process
    printf("Child PID: %d\n", pid);
} else {
    perror("fork failed");
}
```

---

### `execve()` - Execute a program

**Syscall Number**: 59

**Signature**:

```c
int execve(const char *pathname, char *const argv[], char *const envp[]);
```

**Parameters**:

- `pathname`: Path to executable
- `argv`: Argument array (NULL-terminated)
- `envp`: Environment array (NULL-terminated)

**Returns**:

- Does not return on success
- -1 on error (errno set)

**Example**:

```c
char *argv[] = {"/bin/ls", "-l", NULL};
char *envp[] = {NULL};
execve("/bin/ls", argv, envp);
// Only reached if execve fails
perror("execve failed");
```

---

### `exit()` - Terminate process

**Syscall Number**: 60

**Signature**:

```c
void exit(int status);
```

**Parameters**:

- `status`: Exit status code

**Description**: Terminates the calling process and returns status to parent.

---

### `wait4()` - Wait for process state change

**Syscall Number**: 61

**Signature**:

```c
pid_t wait4(pid_t pid, int *wstatus, int options, struct rusage *rusage);
```

**Parameters**:

- `pid`: Process ID to wait for (-1 for any child)
- `wstatus`: Pointer to store exit status
- `options`: Wait options (WNOHANG, WUNTRACED)
- `rusage`: Resource usage information

**Returns**:

- PID of terminated child
- 0 if WNOHANG and child still running
- -1 on error

---

### `getpid()` - Get process ID

**Syscall Number**: 39

**Signature**:

```c
pid_t getpid(void);
```

**Returns**: Current process ID

---

### `getppid()` - Get parent process ID

**Syscall Number**: 110

**Signature**:

```c
pid_t getppid(void);
```

**Returns**: Parent process ID

---

### `gettid()` - Get thread ID

**Syscall Number**: 186

**Signature**:

```c
pid_t gettid(void);
```

**Returns**: Current thread ID

---

## Process Scheduling

### `sched_yield()` - Yield CPU

**Syscall Number**: 24

**Signature**:

```c
int sched_yield(void);
```

**Description**: Causes the calling thread to relinquish the CPU.

**Returns**: 0 on success, -1 on error

---

### `setpriority()` - Set process priority

**Syscall Number**: 141

**Signature**:

```c
int setpriority(int which, id_t who, int prio);
```

**Parameters**:

- `which`: PRIO_PROCESS, PRIO_PGRP, or PRIO_USER
- `who`: ID to set priority for (0 = calling process)
- `prio`: Priority value (-20 to 19, lower = higher priority)

**Returns**: 0 on success, -1 on error

---

### `getpriority()` - Get process priority

**Syscall Number**: 140

**Signature**:

```c
int getpriority(int which, id_t who);
```

**Returns**: Priority value (20-prio), or -1 on error

---

### `sched_setscheduler()` - Set scheduling policy

**Syscall Number**: 144

**Signature**:

```c
int sched_setscheduler(pid_t pid, int policy, const struct sched_param *param);
```

**Parameters**:

- `pid`: Process ID (0 = calling process)
- `policy`: SCHED_OTHER, SCHED_FIFO, SCHED_RR, SCHED_BATCH, SCHED_IDLE
- `param`: Scheduling parameters

---

### `sched_getscheduler()` - Get scheduling policy

**Syscall Number**: 145

**Signature**:

```c
int sched_getscheduler(pid_t pid);
```

**Returns**: Scheduling policy, or -1 on error

---

### `sched_setparam()` - Set scheduling parameters

**Syscall Number**: 142

---

### `sched_getparam()` - Get scheduling parameters

**Syscall Number**: 143

---

## Memory Management

### `brk()` - Change data segment size

**Syscall Number**: 12

**Signature**:

```c
int brk(void *addr);
```

**Description**: Sets the end of the data segment to `addr`.

**Returns**: 0 on success, -1 on error

---

### `mmap()` - Map memory

**Syscall Number**: 9

**Signature**:

```c
void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset);
```

**Parameters**:

- `addr`: Preferred address (NULL for kernel choice)
- `length`: Size to map
- `prot`: PROT_READ | PROT_WRITE | PROT_EXEC
- `flags`: MAP_PRIVATE | MAP_SHARED | MAP_ANONYMOUS
- `fd`: File descriptor (if file-backed)
- `offset`: File offset

**Returns**: Pointer to mapped area, or MAP_FAILED on error

**Example**:

```c
// Anonymous private mapping
void *mem = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
if (mem == MAP_FAILED) {
    perror("mmap");
}
```

---

### `munmap()` - Unmap memory

**Syscall Number**: 11

**Signature**:

```c
int munmap(void *addr, size_t length);
```

**Returns**: 0 on success, -1 on error

---

### `mprotect()` - Change memory protection

**Syscall Number**: 10

**Signature**:

```c
int mprotect(void *addr, size_t len, int prot);
```

**Parameters**:

- `prot`: PROT_NONE | PROT_READ | PROT_WRITE | PROT_EXEC

---

### `madvise()` - Give memory advice

**Syscall Number**: 28

**Signature**:

```c
int madvise(void *addr, size_t length, int advice);
```

**Parameters**:

- `advice`: MADV_NORMAL, MADV_RANDOM, MADV_SEQUENTIAL, MADV_WILLNEED, MADV_DONTNEED

---

### `mincore()` - Check if pages are resident

**Syscall Number**: 27

---

### `mlock()` - Lock memory

**Syscall Number**: 149

---

### `munlock()` - Unlock memory

**Syscall Number**: 150

---

## File I/O

### `open()` - Open file

**Syscall Number**: 2

**Signature**:

```c
int open(const char *pathname, int flags, mode_t mode);
```

**Parameters**:

- `pathname`: File path
- `flags`: O_RDONLY, O_WRONLY, O_RDWR, O_CREAT, O_TRUNC, O_APPEND, O_NONBLOCK
- `mode`: Permissions if creating (0644, etc.)

**Returns**: File descriptor, or -1 on error

**Example**:

```c
int fd = open("/etc/passwd", O_RDONLY);
if (fd < 0) {
    perror("open");
}
```

---

### `close()` - Close file descriptor

**Syscall Number**: 3

**Signature**:

```c
int close(int fd);
```

**Returns**: 0 on success, -1 on error

---

### `read()` - Read from file

**Syscall Number**: 0

**Signature**:

```c
ssize_t read(int fd, void *buf, size_t count);
```

**Returns**: Bytes read, 0 on EOF, -1 on error

**Example**:

```c
char buffer[1024];
ssize_t n = read(fd, buffer, sizeof(buffer));
if (n < 0) {
    perror("read");
} else if (n == 0) {
    printf("EOF\n");
} else {
    printf("Read %zd bytes\n", n);
}
```

---

### `write()` - Write to file

**Syscall Number**: 1

**Signature**:

```c
ssize_t write(int fd, const void *buf, size_t count);
```

**Returns**: Bytes written, or -1 on error

---

### `lseek()` - Reposition file offset

**Syscall Number**: 8

**Signature**:

```c
off_t lseek(int fd, off_t offset, int whence);
```

**Parameters**:

- `whence`: SEEK_SET, SEEK_CUR, SEEK_END

**Returns**: New offset, or -1 on error

---

### `readv()` - Read into multiple buffers

**Syscall Number**: 19

**Signature**:

```c
ssize_t readv(int fd, const struct iovec *iov, int iovcnt);
```

---

### `writev()` - Write from multiple buffers

**Syscall Number**: 20

---

### `dup()` - Duplicate file descriptor

**Syscall Number**: 32

**Signature**:

```c
int dup(int oldfd);
```

**Returns**: New file descriptor, or -1 on error

---

### `dup2()` - Duplicate to specific FD

**Syscall Number**: 33

**Signature**:

```c
int dup2(int oldfd, int newfd);
```

---

### `dup3()` - Duplicate with flags

**Syscall Number**: 292

---

### `fcntl()` - File control

**Syscall Number**: 72

**Signature**:

```c
int fcntl(int fd, int cmd, ... /* arg */);
```

**Commands**: F_GETFD, F_SETFD, F_GETFL, F_SETFL, F_DUPFD

---

## Filesystem Operations

### `mkdir()` - Create directory

**Syscall Number**: 83

**Signature**:

```c
int mkdir(const char *pathname, mode_t mode);
```

**Returns**: 0 on success, -1 on error

---

### `rmdir()` - Remove directory

**Syscall Number**: 84

---

### `getcwd()` - Get current working directory

**Syscall Number**: 79

**Signature**:

```c
char *getcwd(char *buf, size_t size);
```

---

### `chdir()` - Change directory

**Syscall Number**: 80

---

### `fchdir()` - Change directory by FD

**Syscall Number**: 81

---

### `link()` - Create hard link

**Syscall Number**: 86

**Signature**:

```c
int link(const char *oldpath, const char *newpath);
```

---

### `symlink()` - Create symbolic link

**Syscall Number**: 88

**Signature**:

```c
int symlink(const char *target, const char *linkpath);
```

---

### `readlink()` - Read symbolic link

**Syscall Number**: 89

**Signature**:

```c
ssize_t readlink(const char *pathname, char *buf, size_t bufsiz);
```

---

### `unlink()` - Delete file

**Syscall Number**: 87

---

### `unlinkat()` - Delete file (at-variant)

**Syscall Number**: 263

---

### `rename()` - Rename file

**Syscall Number**: 82

---

### `renameat()` - Rename file (at-variant)

**Syscall Number**: 264

---

### `getdents64()` - Read directory entries

**Syscall Number**: 217

**Signature**:

```c
int getdents64(unsigned int fd, struct linux_dirent64 *dirp, unsigned int count);
```

---

### `truncate()` - Truncate file

**Syscall Number**: 76

**Signature**:

```c
int truncate(const char *path, off_t length);
```

---

### `ftruncate()` - Truncate file by FD

**Syscall Number**: 77

---

## File Metadata

### `stat()` - Get file status

**Syscall Number**: 4

**Signature**:

```c
int stat(const char *pathname, struct stat *statbuf);
```

**stat structure**:

```c
struct stat {
    dev_t     st_dev;     // Device ID
    ino_t     st_ino;     // Inode number
    mode_t    st_mode;    // File type and mode
    nlink_t   st_nlink;   // Number of hard links
    uid_t     st_uid;     // User ID
    gid_t     st_gid;     // Group ID
    off_t     st_size;    // Total size in bytes
    time_t    st_atime;   // Last access time
    time_t    st_mtime;   // Last modification time
    time_t    st_ctime;   // Last status change time
};
```

---

### `fstat()` - Get file status by FD

**Syscall Number**: 5

---

### `lstat()` - Get symbolic link status

**Syscall Number**: 6

---

### `chmod()` - Change permissions

**Syscall Number**: 90

**Signature**:

```c
int chmod(const char *pathname, mode_t mode);
```

---

### `fchmod()` - Change permissions by FD

**Syscall Number**: 91

---

### `chown()` - Change owner

**Syscall Number**: 92

---

### `fchown()` - Change owner by FD

**Syscall Number**: 93

---

### `lchown()` - Change owner (no dereference)

**Syscall Number**: 94

---

### `umask()` - Set file creation mask

**Syscall Number**: 95

**Signature**:

```c
mode_t umask(mode_t mask);
```

**Returns**: Previous mask value

---

## File Operations Advanced

### `sync()` - Sync filesystem

**Syscall Number**: 162

**Signature**:

```c
void sync(void);
```

**Description**: Flushes filesystem buffers to disk.

---

### `fsync()` - Sync file

**Syscall Number**: 74

**Signature**:

```c
int fsync(int fd);
```

---

### `fdatasync()` - Sync file data

**Syscall Number**: 75

---

### `sendfile()` - Transfer data between FDs

**Syscall Number**: 40

**Signature**:

```c
ssize_t sendfile(int out_fd, int in_fd, off_t *offset, size_t count);
```

**Description**: Efficient zero-copy data transfer.

---

### `splice()` - Splice data to/from pipe

**Syscall Number**: 275

**Signature**:

```c
ssize_t splice(int fd_in, off_t *off_in, int fd_out, off_t *off_out,
               size_t len, unsigned int flags);
```

---

### `tee()` - Duplicate pipe content

**Syscall Number**: 276

---

### `ioctl()` - Device control

**Syscall Number**: 16

**Signature**:

```c
int ioctl(int fd, unsigned long request, ...);
```

---

### `select()` - Synchronous I/O multiplexing

**Syscall Number**: 23

---

## Pipes & FIFOs

### `pipe()` - Create pipe

**Syscall Number**: 22

**Signature**:

```c
int pipe(int pipefd[2]);
```

**Description**: Creates a unidirectional pipe. `pipefd[0]` is read end, `pipefd[1]` is write end.

**Example**:

```c
int pipefd[2];
if (pipe(pipefd) == -1) {
    perror("pipe");
}

if (fork() == 0) {
    // Child reads
    close(pipefd[1]);
    char buf[100];
    read(pipefd[0], buf, sizeof(buf));
} else {
    // Parent writes
    close(pipefd[0]);
    write(pipefd[1], "Hello", 5);
}
```

---

### `pipe2()` - Create pipe with flags

**Syscall Number**: 293

**Signature**:

```c
int pipe2(int pipefd[2], int flags);
```

**Flags**: O_NONBLOCK, O_CLOEXEC

---

### `mkfifo()` - Create named pipe

**Syscall Number**: (via mknod)

---

## Signals

### `sigaction()` - Set signal action

**Syscall Number**: 13

**Signature**:

```c
int sigaction(int signum, const struct sigaction *act, struct sigaction *oldact);
```

**sigaction structure**:

```c
struct sigaction {
    void (*sa_handler)(int);
    void (*sa_sigaction)(int, siginfo_t *, void *);
    sigset_t sa_mask;
    int sa_flags;
};
```

**Example**:

```c
struct sigaction sa;
sa.sa_handler = signal_handler;
sigemptyset(&sa.sa_mask);
sa.sa_flags = 0;
sigaction(SIGINT, &sa, NULL);
```

---

### `sigprocmask()` - Set signal mask

**Syscall Number**: 14

**Signature**:

```c
int sigprocmask(int how, const sigset_t *set, sigset_t *oldset);
```

**Parameters**:

- `how`: SIG_BLOCK, SIG_UNBLOCK, SIG_SETMASK

---

### `kill()` - Send signal to process

**Syscall Number**: 62

**Signature**:

```c
int kill(pid_t pid, int sig);
```

---

### `tkill()` - Send signal to thread

**Syscall Number**: 200

---

### `rt_sigpending()` - Check pending signals

**Syscall Number**: 127

---

### `rt_sigsuspend()` - Wait for signal

**Syscall Number**: 130

---

### `sigaltstack()` - Set alternate signal stack

**Syscall Number**: 131

---

### `rt_sigreturn()` - Return from signal handler

**Syscall Number**: 15

---

## Polling & Events

### `poll()` - Wait for events

**Syscall Number**: 7

**Signature**:

```c
int poll(struct pollfd *fds, nfds_t nfds, int timeout);
```

**pollfd structure**:

```c
struct pollfd {
    int fd;         // File descriptor
    short events;   // Requested events
    short revents;  // Returned events
};
```

**Events**: POLLIN, POLLOUT, POLLERR, POLLHUP

---

### `epoll_create()` - Create epoll instance

**Syscall Number**: 213

---

### `epoll_ctl()` - Control epoll

**Syscall Number**: 233

**Signature**:

```c
int epoll_ctl(int epfd, int op, int fd, struct epoll_event *event);
```

**Operations**: EPOLL_CTL_ADD, EPOLL_CTL_MOD, EPOLL_CTL_DEL

---

### `epoll_wait()` - Wait for epoll events

**Syscall Number**: 232

**Signature**:

```c
int epoll_wait(int epfd, struct epoll_event *events, int maxevents, int timeout);
```

---

## Sockets & Networking

### `socket()` - Create socket

**Syscall Number**: 41

**Signature**:

```c
int socket(int domain, int type, int protocol);
```

**Domains**: AF_INET, AF_INET6, AF_UNIX
**Types**: SOCK_STREAM, SOCK_DGRAM, SOCK_RAW

**Example**:

```c
// TCP socket
int fd = socket(AF_INET, SOCK_STREAM, 0);

// Unix domain socket
int ufd = socket(AF_UNIX, SOCK_STREAM, 0);
```

---

### `bind()` - Bind socket to address

**Syscall Number**: 49

**Signature**:

```c
int bind(int sockfd, const struct sockaddr *addr, socklen_t addrlen);
```

---

### `listen()` - Listen for connections

**Syscall Number**: 50

**Signature**:

```c
int listen(int sockfd, int backlog);
```

---

### `accept()` - Accept connection

**Syscall Number**: 43

**Signature**:

```c
int accept(int sockfd, struct sockaddr *addr, socklen_t *addrlen);
```

---

### `connect()` - Connect to socket

**Syscall Number**: 42

---

### `send()` - Send data

**Syscall Number**: 44 (via sendto)

---

### `recv()` - Receive data

**Syscall Number**: 45 (via recvfrom)

---

### `sendto()` - Send data to address

**Syscall Number**: 44

---

### `recvfrom()` - Receive data from address

**Syscall Number**: 45

---

### `socketpair()` - Create socket pair

**Syscall Number**: 53

---

### `shutdown()` - Shut down socket

**Syscall Number**: 48

---

### `sendmsg()` - Send message

**Syscall Number**: 46

---

### `recvmsg()` - Receive message

**Syscall Number**: 47

---

### `getsockopt()` - Get socket options

**Syscall Number**: 55

---

### `setsockopt()` - Set socket options

**Syscall Number**: 54

---

## System V IPC

### `shmget()` - Get shared memory segment

**Syscall Number**: 29

**Signature**:

```c
int shmget(key_t key, size_t size, int shmflg);
```

---

### `shmat()` - Attach shared memory

**Syscall Number**: 30

**Signature**:

```c
void *shmat(int shmid, const void *shmaddr, int shmflg);
```

---

### `shmdt()` - Detach shared memory

**Syscall Number**: 67

---

### `shmctl()` - Control shared memory

**Syscall Number**: 31

---

### `semget()` - Get semaphore set

**Syscall Number**: 64

---

### `semop()` - Semaphore operations

**Syscall Number**: 65

---

### `semctl()` - Control semaphores

**Syscall Number**: 66

---

### `msgget()` - Get message queue

**Syscall Number**: 68

---

### `msgsnd()` - Send message

**Syscall Number**: 69

---

### `msgrcv()` - Receive message

**Syscall Number**: 70

---

### `msgctl()` - Control message queue

**Syscall Number**: 71

---

## Event File Descriptors

### `eventfd()` - Create event FD

**Syscall Number**: 284

**Signature**:

```c
int eventfd(unsigned int initval, int flags);
```

**Description**: Creates a file descriptor for event notification.

---

### `signalfd()` - Create signal FD

**Syscall Number**: 282

**Signature**:

```c
int signalfd(int fd, const sigset_t *mask, int flags);
```

**Description**: Creates a file descriptor that receives signals.

---

## Process Limits

### `getrlimit()` - Get resource limits

**Syscall Number**: 97

**Signature**:

```c
int getrlimit(int resource, struct rlimit *rlim);
```

**Resources**: RLIMIT_CPU, RLIMIT_FSIZE, RLIMIT_DATA, RLIMIT_STACK, RLIMIT_NOFILE

---

### `setrlimit()` - Set resource limits

**Syscall Number**: 160

---

### `prlimit64()` - Get/set resource limits

**Syscall Number**: 302

---

### `getrusage()` - Get resource usage

**Syscall Number**: 98

**Signature**:

```c
int getrusage(int who, struct rusage *usage);
```

**who**: RUSAGE_SELF, RUSAGE_CHILDREN

---

## File Notifications

### `inotify_init()` - Initialize inotify

**Syscall Number**: 253

**Signature**:

```c
int inotify_init(void);
```

**Returns**: inotify file descriptor

---

### `inotify_init1()` - Initialize with flags

**Syscall Number**: 294

---

### `inotify_add_watch()` - Add watch

**Syscall Number**: 254

**Signature**:

```c
int inotify_add_watch(int fd, const char *pathname, uint32_t mask);
```

**Events**: IN_ACCESS, IN_MODIFY, IN_CREATE, IN_DELETE, IN_OPEN, IN_CLOSE

---

### `inotify_rm_watch()` - Remove watch

**Syscall Number**: 255

---

## System Information

### `uname()` - Get system information

**Syscall Number**: 63

**Signature**:

```c
int uname(struct utsname *buf);
```

**utsname structure**:

```c
struct utsname {
    char sysname[65];    // "Exo-OS"
    char nodename[65];   // Hostname
    char release[65];    // "0.2.0"
    char version[65];    // Version info
    char machine[65];    // "x86_64"
    char domainname[65]; // Domain name
};
```

---

### `sysinfo()` - Get system statistics

**Syscall Number**: 99

**Signature**:

```c
int sysinfo(struct sysinfo *info);
```

---

### `gettimeofday()` - Get time

**Syscall Number**: 96

---

### `getrandom()` - Get random bytes

**Syscall Number**: 318

**Signature**:

```c
ssize_t getrandom(void *buf, size_t buflen, unsigned int flags);
```

---

## Security & Capabilities

### `getuid()` - Get user ID

**Syscall Number**: 102

---

### `getgid()` - Get group ID

**Syscall Number**: 104

---

### `geteuid()` - Get effective user ID

**Syscall Number**: 107

---

### `getegid()` - Get effective group ID

**Syscall Number**: 108

---

### `setuid()` - Set user ID

**Syscall Number**: 105

---

### `setgid()` - Set group ID

**Syscall Number**: 106

---

### `prctl()` - Process control

**Syscall Number**: 157

**Signature**:

```c
int prctl(int option, unsigned long arg2, unsigned long arg3,
          unsigned long arg4, unsigned long arg5);
```

**Options**:

- PR_SET_NAME / PR_GET_NAME - Process name
- PR_SET_DUMPABLE / PR_GET_DUMPABLE - Core dump flag
- PR_SET_PDEATHSIG / PR_GET_PDEATHSIG - Parent death signal
- PR_SET_SECCOMP / PR_GET_SECCOMP - Seccomp mode

**Example**:

```c
// Set process name
prctl(PR_SET_NAME, "my-daemon", 0, 0, 0);

// Get process name
char name[16];
prctl(PR_GET_NAME, name, 0, 0, 0);
```

---

## Summary

This reference documents all **127 implemented syscalls** in POSIX-X, organized by functional category. Each syscall follows Linux/POSIX semantics and provides comprehensive error handling.

For implementation details, see module-specific documentation:

- [VFS Guide](VFS_GUIDE.md)
- [IPC Guide](IPC_GUIDE.md)
- [Process Guide](PROCESS_GUIDE.md)
- [Signal Guide](SIGNAL_GUIDE.md)
- [Network Guide](NETWORK_GUIDE.md)

For architecture overview, see [ARCHITECTURE.md](ARCHITECTURE.md).
