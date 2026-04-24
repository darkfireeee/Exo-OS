//! # syscall/numbers.rs — Numéros d'appels système Exo-OS
//!
//! Définit les constantes numériques de tous les syscalls supportés.
//!
//! ## Compatibilité
//! - [0..299]   : numéros compatibles Linux x86_64 (même ABI que glibc)
//! - [300..399] : extensions Exo-OS (IPC natif, capabilities, sécurité)
//! - [400..511] : réservés pour usage futur
//! - 512        : SYSCALL_TABLE_SIZE (taille totale de la table)
//!
//! ## Règle architecturale
//! Les numéros Linux sont repris à l'identique pour permettre une libc
//! musl/glibc sans patch. Les syscalls Exo-OS natifs commencent à 300
//! pour éviter tout conflit avec de futurs ajouts Linux.
//!
//! ## ABI Registres (Linux/Exo-OS 64-bit)
//! ```
//! rax = numéro syscall (entrée) / valeur retour (sortie)
//! rdi = arg1   rsi = arg2   rdx = arg3
//! r10 = arg4   r8  = arg5   r9  = arg6
//! rcx = RIP retour (sauvé par SYSCALL hardware)
//! r11 = RFLAGS (sauvé par SYSCALL hardware)
//! ```

// ─────────────────────────────────────────────────────────────────────────────
// Taille totale de la table de dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// Taille de la table syscall (un slot par numéro possible).
/// 547 = couvre POSIX (0–499) + ExoFS (500–520) + GI-03 drivers (530–546).
pub const SYSCALL_TABLE_SIZE: usize = 547;

/// Numéro invalide (retourne -ENOSYS)
pub const SYSCALL_INVALID: u64 = u64::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// Bloc 0–99 : I/O, Fichiers, Mémoire (Linux-compatible)
// ─────────────────────────────────────────────────────────────────────────────

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_LSTAT: u64 = 6;
pub const SYS_POLL: u64 = 7;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_RT_SIGRETURN: u64 = 15;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_PREAD64: u64 = 17;
pub const SYS_PWRITE64: u64 = 18;
pub const SYS_READV: u64 = 19;
pub const SYS_WRITEV: u64 = 20;
pub const SYS_ACCESS: u64 = 21;
pub const SYS_PIPE: u64 = 22;
pub const SYS_SELECT: u64 = 23;
pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_MREMAP: u64 = 25;
pub const SYS_MSYNC: u64 = 26;
pub const SYS_MINCORE: u64 = 27;
pub const SYS_MADVISE: u64 = 28;
pub const SYS_SHMGET: u64 = 29;
pub const SYS_SHMAT: u64 = 30;
pub const SYS_SHMCTL: u64 = 31;
pub const SYS_DUP: u64 = 32;
pub const SYS_DUP2: u64 = 33;
pub const SYS_PAUSE: u64 = 34;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_GETITIMER: u64 = 36;
pub const SYS_ALARM: u64 = 37;
pub const SYS_SETITIMER: u64 = 38;
pub const SYS_GETPID: u64 = 39;
pub const SYS_SENDFILE: u64 = 40;
pub const SYS_SOCKET: u64 = 41;
pub const SYS_CONNECT: u64 = 42;
pub const SYS_ACCEPT: u64 = 43;
pub const SYS_SENDTO: u64 = 44;
pub const SYS_RECVFROM: u64 = 45;
pub const SYS_SENDMSG: u64 = 46;
pub const SYS_RECVMSG: u64 = 47;
pub const SYS_SHUTDOWN: u64 = 48;
pub const SYS_BIND: u64 = 49;
pub const SYS_LISTEN: u64 = 50;
pub const SYS_GETSOCKNAME: u64 = 51;
pub const SYS_GETPEERNAME: u64 = 52;
pub const SYS_SOCKETPAIR: u64 = 53;
pub const SYS_SETSOCKOPT: u64 = 54;
pub const SYS_GETSOCKOPT: u64 = 55;
pub const SYS_CLONE: u64 = 56;
pub const SYS_FORK: u64 = 57;
pub const SYS_VFORK: u64 = 58;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAIT4: u64 = 61;
pub const SYS_KILL: u64 = 62;
pub const SYS_UNAME: u64 = 63;
pub const SYS_SEMGET: u64 = 64;
pub const SYS_SEMOP: u64 = 65;
pub const SYS_SEMCTL: u64 = 66;
pub const SYS_SHMDT: u64 = 67;
pub const SYS_MSGGET: u64 = 68;
pub const SYS_MSGSND: u64 = 69;
pub const SYS_MSGRCV: u64 = 70;
pub const SYS_MSGCTL: u64 = 71;
pub const SYS_FCNTL: u64 = 72;
pub const SYS_FLOCK: u64 = 73;
pub const SYS_FSYNC: u64 = 74;
pub const SYS_FDATASYNC: u64 = 75;
pub const SYS_TRUNCATE: u64 = 76;
pub const SYS_FTRUNCATE: u64 = 77;
pub const SYS_GETDENTS: u64 = 78;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_FCHDIR: u64 = 81;
pub const SYS_RENAME: u64 = 82;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_CREAT: u64 = 85;
pub const SYS_LINK: u64 = 86;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_SYMLINK: u64 = 88;
pub const SYS_READLINK: u64 = 89;
pub const SYS_CHMOD: u64 = 90;
pub const SYS_FCHMOD: u64 = 91;
pub const SYS_CHOWN: u64 = 92;
pub const SYS_FCHOWN: u64 = 93;
pub const SYS_LCHOWN: u64 = 94;
pub const SYS_UMASK: u64 = 95;
pub const SYS_GETTIMEOFDAY: u64 = 96;
pub const SYS_GETRLIMIT: u64 = 97;
pub const SYS_GETRUSAGE: u64 = 98;
pub const SYS_SYSINFO: u64 = 99;

// ─────────────────────────────────────────────────────────────────────────────
// Bloc 100–199 : Temps, Processus, Signaux (Linux-compatible)
// ─────────────────────────────────────────────────────────────────────────────

pub const SYS_TIMES: u64 = 100;
pub const SYS_PTRACE: u64 = 101;
pub const SYS_GETUID: u64 = 102;
pub const SYS_SYSLOG: u64 = 103;
pub const SYS_GETGID: u64 = 104;
pub const SYS_SETUID: u64 = 105;
pub const SYS_SETGID: u64 = 106;
pub const SYS_GETEUID: u64 = 107;
pub const SYS_GETEGID: u64 = 108;
pub const SYS_SETPGID: u64 = 109;
pub const SYS_GETPPID: u64 = 110;
pub const SYS_GETPGRP: u64 = 111;
pub const SYS_SETSID: u64 = 112;
pub const SYS_SETREUID: u64 = 113;
pub const SYS_SETREGID: u64 = 114;
pub const SYS_GETGROUPS: u64 = 115;
pub const SYS_SETGROUPS: u64 = 116;
pub const SYS_SETRESUID: u64 = 117;
pub const SYS_GETRESUID: u64 = 118;
pub const SYS_SETRESGID: u64 = 119;
pub const SYS_GETRESGID: u64 = 120;
pub const SYS_GETPGID: u64 = 121;
pub const SYS_SETFSUID: u64 = 122;
pub const SYS_SETFSGID: u64 = 123;
pub const SYS_GETSID: u64 = 124;
pub const SYS_CAPGET: u64 = 125;
pub const SYS_CAPSET: u64 = 126;
pub const SYS_RT_SIGPENDING: u64 = 127;
pub const SYS_RT_SIGTIMEDWAIT: u64 = 128;
pub const SYS_RT_SIGQUEUEINFO: u64 = 129;
pub const SYS_RT_SIGSUSPEND: u64 = 130;
pub const SYS_SIGALTSTACK: u64 = 131;
pub const SYS_UTIME: u64 = 132;
pub const SYS_MKNOD: u64 = 133;
pub const SYS_USELIB: u64 = 134;
pub const SYS_PERSONALITY: u64 = 135;
pub const SYS_USTAT: u64 = 136;
pub const SYS_STATFS: u64 = 137;
pub const SYS_FSTATFS: u64 = 138;
pub const SYS_SYSFS: u64 = 139;
pub const SYS_GETPRIORITY: u64 = 140;
pub const SYS_SETPRIORITY: u64 = 141;
pub const SYS_SCHED_SETPARAM: u64 = 142;
pub const SYS_SCHED_GETPARAM: u64 = 143;
pub const SYS_SCHED_SETSCHEDULER: u64 = 144;
pub const SYS_SCHED_GETSCHEDULER: u64 = 145;
pub const SYS_SCHED_GET_PRIORITY_MAX: u64 = 146;
pub const SYS_SCHED_GET_PRIORITY_MIN: u64 = 147;
pub const SYS_SCHED_RR_GET_INTERVAL: u64 = 148;
pub const SYS_MLOCK: u64 = 149;
pub const SYS_MUNLOCK: u64 = 150;
pub const SYS_MLOCKALL: u64 = 151;
pub const SYS_MUNLOCKALL: u64 = 152;
pub const SYS_VHANGUP: u64 = 153;
pub const SYS_MODIFY_LDT: u64 = 154;
pub const SYS_PIVOT_ROOT: u64 = 155;
pub const SYS_PRCTL: u64 = 157;
pub const SYS_ARCH_PRCTL: u64 = 158;
pub const SYS_ADJTIMEX: u64 = 159;
pub const SYS_SETRLIMIT: u64 = 160;
pub const SYS_CHROOT: u64 = 161;
pub const SYS_SYNC: u64 = 162;
pub const SYS_ACCT: u64 = 163;
pub const SYS_SETTIMEOFDAY: u64 = 164;
pub const SYS_MOUNT: u64 = 165;
pub const SYS_UMOUNT2: u64 = 166;
pub const SYS_SWAPON: u64 = 167;
pub const SYS_SWAPOFF: u64 = 168;
pub const SYS_REBOOT: u64 = 169;
pub const SYS_SETHOSTNAME: u64 = 170;
pub const SYS_SETDOMAINNAME: u64 = 171;
pub const SYS_IOPL: u64 = 172;
pub const SYS_IOPERM: u64 = 173;
pub const SYS_CREATE_MODULE: u64 = 174;
pub const SYS_INIT_MODULE: u64 = 175;
pub const SYS_DELETE_MODULE: u64 = 176;
pub const SYS_QUERY_MODULE: u64 = 177;
pub const SYS_QUOTACTL: u64 = 179;
pub const SYS_GETTID: u64 = 186;
pub const SYS_TKILL: u64 = 200;
pub const SYS_TIME: u64 = 201;
pub const SYS_FUTEX: u64 = 202;
pub const SYS_SCHED_SETAFFINITY: u64 = 203;
pub const SYS_SCHED_GETAFFINITY: u64 = 204;
pub const SYS_EPOLL_CREATE: u64 = 213;
pub const SYS_GETDENTS64: u64 = 217;
pub const SYS_SET_TID_ADDRESS: u64 = 218;
pub const SYS_SEMTIMEDOP: u64 = 220;
pub const SYS_FADVISE64: u64 = 221;
pub const SYS_TIMER_CREATE: u64 = 222;
pub const SYS_TIMER_SETTIME: u64 = 223;
pub const SYS_TIMER_GETTIME: u64 = 224;
pub const SYS_TIMER_GETOVERRUN: u64 = 225;
pub const SYS_TIMER_DELETE: u64 = 226;
pub const SYS_CLOCK_SETTIME: u64 = 227;
pub const SYS_CLOCK_GETTIME: u64 = 228;
pub const SYS_CLOCK_GETRES: u64 = 229;
pub const SYS_CLOCK_NANOSLEEP: u64 = 230;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_EPOLL_WAIT: u64 = 232;
pub const SYS_EPOLL_CTL: u64 = 233;
pub const SYS_TGKILL: u64 = 234;
pub const SYS_UTIMES: u64 = 235;
pub const SYS_WAITID: u64 = 247;
pub const SYS_OPENAT: u64 = 257;
pub const SYS_MKDIRAT: u64 = 258;
pub const SYS_MKNODAT: u64 = 259;
pub const SYS_FCHOWNAT: u64 = 260;
pub const SYS_FUTIMESAT: u64 = 261;
pub const SYS_NEWFSTATAT: u64 = 262;
pub const SYS_UNLINKAT: u64 = 263;
pub const SYS_RENAMEAT: u64 = 264;
pub const SYS_LINKAT: u64 = 265;
pub const SYS_SYMLINKAT: u64 = 266;
pub const SYS_READLINKAT: u64 = 267;
pub const SYS_FCHMODAT: u64 = 268;
pub const SYS_FACCESSAT: u64 = 269;
pub const SYS_PSELECT6: u64 = 270;
pub const SYS_PPOLL: u64 = 271;
pub const SYS_UNSHARE: u64 = 272;
pub const SYS_SPLICE: u64 = 275;
pub const SYS_TEE: u64 = 276;
pub const SYS_VMSPLICE: u64 = 278;
pub const SYS_GETCPU: u64 = 309; // conflit: remappé en 298 côté Linux, voir compat

// ─────────────────────────────────────────────────────────────────────────────
// Bloc 300–399 : Syscalls natifs Exo-OS
// ─────────────────────────────────────────────────────────────────────────────

/// Envoyer un message IPC natif Exo-OS
pub const SYS_EXO_IPC_SEND: u64 = 300;
/// Recevoir un message IPC natif Exo-OS
pub const SYS_EXO_IPC_RECV: u64 = 301;
/// Recevoir en mode non-bloquant
pub const SYS_EXO_IPC_RECV_NB: u64 = 302;
/// Appel IPC synchrone (send + recv atomique)
pub const SYS_EXO_IPC_CALL: u64 = 303;
/// Créer un endpoint IPC
pub const SYS_EXO_IPC_CREATE: u64 = 304;
/// Détruire un endpoint IPC
pub const SYS_EXO_IPC_DESTROY: u64 = 305;
/// Partager une page de mémoire via capability
pub const SYS_EXO_MEM_SHARE: u64 = 310;
/// Révoquer un partage mémoire
pub const SYS_EXO_MEM_REVOKE: u64 = 311;
/// Obtenir une capability
pub const SYS_EXO_CAP_CREATE: u64 = 320;
/// Déléguer une capability à un autre processus
pub const SYS_EXO_CAP_DELEGATE: u64 = 321;
/// Révoquer une capability
pub const SYS_EXO_CAP_REVOKE: u64 = 322;
/// Vérifier si une capability est valide
pub const SYS_EXO_CAP_CHECK: u64 = 323;
/// Lire un compteur de performance kernel
pub const SYS_EXO_PERF_READ: u64 = 330;
/// Activer les événements de performance
pub const SYS_EXO_PERF_ENABLE: u64 = 331;
/// Désactiver les événements de performance
pub const SYS_EXO_PERF_DISABLE: u64 = 332;
/// Activer/désactiver le mode debug d'un processus (Ring 0 uniquement)
pub const SYS_EXO_DEBUG_ATTACH: u64 = 340;
/// Lire les registres d'un processus en debug
pub const SYS_EXO_DEBUG_REGS: u64 = 341;
/// Log kernel direct (ring 0 permissions requises)
pub const SYS_EXO_LOG: u64 = 350;
/// Sonde eBPF Exo-OS
pub const SYS_EXO_BPF: u64 = 360;

// ─────────────────────────────────────────────────────────────────────────────
// Bloc 500–518 : ExoFS natif (filesystem objet ExoOS)
// ─────────────────────────────────────────────────────────────────────────────
// RÈGLE SYS-10 : Syscalls 0-499 = POSIX standard. Syscalls 500-518 = ExoFS natif.
// Ces constantes sont PUBLIQUES — utilisées par exo-libc ET exo-rt.

/// ExoFS : résolution de chemin → ObjectId   (path → BlobId + ObjectId)
pub const SYS_EXOFS_PATH_RESOLVE: u64 = 500;
/// ExoFS : ouverture d'un objet existant      (ObjectId + droits → fd)
pub const SYS_EXOFS_OBJECT_OPEN: u64 = 501;
/// ExoFS : lecture d'un objet                 (fd, offset, buf, len)
pub const SYS_EXOFS_OBJECT_READ: u64 = 502;
/// ExoFS : écriture dans un objet             (fd, offset, buf, len)
pub const SYS_EXOFS_OBJECT_WRITE: u64 = 503;
/// ExoFS : création d'un nouvel objet
pub const SYS_EXOFS_OBJECT_CREATE: u64 = 504;
/// ExoFS : suppression d'un objet
pub const SYS_EXOFS_OBJECT_DELETE: u64 = 505;
/// ExoFS : métadonnées d'un objet (stat-like)
pub const SYS_EXOFS_OBJECT_STAT: u64 = 506;
/// ExoFS : mise à jour des métadonnées
pub const SYS_EXOFS_OBJECT_SET_META: u64 = 507;
/// ExoFS : hash du contenu (audité SEC-09 — copy_from_user obligatoire)
pub const SYS_EXOFS_GET_CONTENT_HASH: u64 = 508;
/// ExoFS : création d'un snapshot
pub const SYS_EXOFS_SNAPSHOT_CREATE: u64 = 509;
/// ExoFS : liste des snapshots disponibles
pub const SYS_EXOFS_SNAPSHOT_LIST: u64 = 510;
/// ExoFS : montage d'un snapshot en lecture seule
pub const SYS_EXOFS_SNAPSHOT_MOUNT: u64 = 511;
/// ExoFS : création d'une relation entre objets
pub const SYS_EXOFS_RELATION_CREATE: u64 = 512;
/// ExoFS : requête sur les relations
pub const SYS_EXOFS_RELATION_QUERY: u64 = 513;
/// ExoFS : déclenchement manuel du GC (garbage collector)
pub const SYS_EXOFS_GC_TRIGGER: u64 = 514;
/// ExoFS : requête de quota capability
pub const SYS_EXOFS_QUOTA_QUERY: u64 = 515;
/// ExoFS : export d'un objet vers userspace
pub const SYS_EXOFS_EXPORT_OBJECT: u64 = 516;
/// ExoFS : import d'un objet depuis userspace
pub const SYS_EXOFS_IMPORT_OBJECT: u64 = 517;
/// ExoFS : commit d'une epoch (3 barrières NVMe — atomicité garantie)
pub const SYS_EXOFS_EPOCH_COMMIT: u64 = 518;

// ─────────────────────────────────────────────────────────────────────────────
// Bloc 519–520 : Extensions ExoFS — correctifs BUG-01/BUG-02
// ─────────────────────────────────────────────────────────────────────────────

/// FIX BUG-01 : open() POSIX combiné Ring0 — enchaîne path_resolve() + object_open()
/// atomiquement. Utilisé par musl-exo : #define __NR_open 519
/// Signature : (path_ptr, path_len, flags, mode) → fd
pub const SYS_EXOFS_OPEN_BY_PATH: u64 = 519;

/// FIX BUG-02 : getdents64 ExoFS — list le contenu d'un répertoire.
/// Utilisé par ls, find, opendir(). Sans ce syscall : ls/find/opendir() impossibles.
/// Signature : (fd, buf_ptr, buf_len) → octets remplis
pub const SYS_EXOFS_READDIR: u64 = 520;

// ─────────────────────────────────────────────────────────────────────────────
// Bloc 530–546 : GI-03 Drivers (IRQ / DMA / PCI / IOMMU)
// ─────────────────────────────────────────────────────────────────────────────

/// GI-03 : enregistrement d'un routage IRQ canonique.
pub const SYS_IRQ_REGISTER: u64 = 530;
/// GI-03 : acquittement d'une IRQ traitée.
pub const SYS_IRQ_ACK: u64 = 531;
/// GI-03 : mapping MMIO (PID appelant, claim requis).
pub const SYS_MMIO_MAP: u64 = 532;
/// GI-03 : unmapping MMIO (PID appelant).
pub const SYS_MMIO_UNMAP: u64 = 533;
/// GI-03 : allocation DMA (retourne IOVA, virt CPU optionnel via out ptr).
pub const SYS_DMA_ALLOC: u64 = 534;
/// GI-03 : libération DMA allouée.
pub const SYS_DMA_FREE: u64 = 535;
/// GI-03 : synchronisation DMA CPU/device.
pub const SYS_DMA_SYNC: u64 = 536;
/// GI-03 : lecture config PCI (device claimé par PID appelant).
pub const SYS_PCI_CFG_READ: u64 = 537;
/// GI-03 : écriture config PCI (device claimé par PID appelant).
pub const SYS_PCI_CFG_WRITE: u64 = 538;
/// GI-03 : contrôle Bus Master PCI (device claimé par PID appelant).
pub const SYS_PCI_BUS_MASTER: u64 = 539;
/// GI-03 : claim PCI sécurisé (CORR-32).
pub const SYS_PCI_CLAIM: u64 = 540;
/// GI-03 : map DMA d'un buffer vers IOVA.
pub const SYS_DMA_MAP: u64 = 541;
/// GI-03 : unmap DMA IOVA.
pub const SYS_DMA_UNMAP: u64 = 542;
/// GI-03 : allocation MSI/MSI-X (handle opaque).
pub const SYS_MSI_ALLOC: u64 = 543;
/// GI-03 : configuration MSI par index de vecteur.
pub const SYS_MSI_CONFIG: u64 = 544;
/// GI-03 : libération d'un handle MSI.
pub const SYS_MSI_FREE: u64 = 545;
/// GI-03 : association topologie PCI (owner/parenting).
pub const SYS_PCI_SET_TOPOLOGY: u64 = 546;

// ─────────────────────────────────────────────────────────────────────────────
// FIX BUG-03 : Aliases process pour exo-rt
// ─────────────────────────────────────────────────────────────────────────────
// exo-rt référence SYS_PROC_CLONE et SYS_PROC_EXEC mais numbers.rs listait
// fork=57 et execve=59 sans alias. Ces constantes sont OBLIGATOIRES pour que
// exo-rt compile.

/// Alias exo-rt pour fork() avec CoW kernel — identique à SYS_FORK (57).
pub const SYS_PROC_CLONE: u64 = SYS_FORK;
/// Alias exo-rt pour execve() — identique à SYS_EXECVE (59).
pub const SYS_PROC_EXEC: u64 = SYS_EXECVE;

// ─────────────────────────────────────────────────────────────────────────────
// Codes d'erreur Exo-OS (compatibles Linux errno)
// ─────────────────────────────────────────────────────────────────────────────

/// Opération non supportée
pub const ENOSYS: i64 = -38;
/// Argument invalide
pub const EINVAL: i64 = -22;
/// Accès refusé
pub const EACCES: i64 = -13;
/// Pointeur invalide / mauvaise adresse
pub const EFAULT: i64 = -14;
/// Trop grand
pub const E2BIG: i64 = -7;
/// Ressource occupée
pub const EBUSY: i64 = -16;
/// Interruption par signal
pub const EINTR: i64 = -4;
/// Mauvais descripteur de fichier
pub const EBADF: i64 = -9;
/// Tentative sur un objet inexistant
pub const ENOENT: i64 = -2;
/// Mémoire insuffisante
pub const ENOMEM: i64 = -12;
/// Opération serait bloquante (non-bloquant demandé)
pub const EAGAIN: i64 = -11;
/// Limite de descripteurs atteinte
pub const EMFILE: i64 = -24;
/// Capacité non disponible / refusée
pub const EPERM: i64 = -1;
/// Dépassement de capacité numérique
pub const EOVERFLOW: i64 = -75;
/// Fichier existe déjà
pub const EEXIST: i64 = -17;
/// N'est pas un répertoire
pub const ENOTDIR: i64 = -20;
/// Est un répertoire
pub const EISDIR: i64 = -21;
/// Espace insuffisant sur le périphérique
pub const ENOSPC: i64 = -28;
/// Opération non supportée
pub const ENOTSUP: i64 = -95;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers de classification
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie si un numéro syscall est dans la plage Linux-compatible
#[inline(always)]
pub const fn is_linux_compat(nr: u64) -> bool {
    nr < 300
}

/// Vérifie si un numéro syscall est natif Exo-OS
#[inline(always)]
pub const fn is_exoos_native(nr: u64) -> bool {
    nr >= 300 && nr < 400
}

/// Vérifie si un numéro est dans la table valide
#[inline(always)]
pub const fn is_valid_syscall(nr: u64) -> bool {
    (nr as usize) < SYSCALL_TABLE_SIZE
}

/// Vérifie si un numéro syscall est ExoFS natif (500–520)
#[inline(always)]
pub const fn is_exofs_syscall(nr: u64) -> bool {
    nr >= SYS_EXOFS_PATH_RESOLVE && nr <= SYS_EXOFS_READDIR
}

/// Numéros de syscall candidats au fast-path (<100 cycles, pas d'alloc, no-lock)
/// Utilisé par dispatch.rs pour court-circuiter la table principale.
pub const FAST_PATH_SYSCALLS: &[u64] = &[
    SYS_GETPID,
    SYS_GETTID,
    SYS_GETUID,
    SYS_GETEUID,
    SYS_GETGID,
    SYS_GETEGID,
    SYS_GETPPID,
    SYS_CLOCK_GETTIME,
    SYS_GETTIMEOFDAY,
    SYS_GETCPU,
    SYS_SCHED_YIELD,
];
