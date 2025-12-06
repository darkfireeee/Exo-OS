# 🔗 INTEGRATION - Guide d'Intégration Filesystem

## 📋 Vue d'Ensemble

Ce guide explique comment intégrer et utiliser le système de fichiers d'Exo-OS.

---

## 1. Initialisation du Filesystem

### Au Boot du Kernel

```rust
// kernel/src/main.rs

fn kernel_main() {
    // 1. Initialiser memory subsystem
    memory::init();
    
    // 2. Initialiser filesystem subsystem
    fs::init();
    
    // 3. Monter root filesystem
    fs::vfs::mount("/", "ext4", "/dev/sda1", MountFlags::empty()).unwrap();
    
    // 4. Monter pseudo-filesystems
    fs::vfs::mount("/dev", "devfs", None, MountFlags::empty()).unwrap();
    fs::vfs::mount("/proc", "procfs", None, MountFlags::empty()).unwrap();
    fs::vfs::mount("/sys", "sysfs", None, MountFlags::empty()).unwrap();
    fs::vfs::mount("/tmp", "tmpfs", None, MountFlags::empty()).unwrap();
    
    log::info!("Filesystem initialized successfully");
}
```

---

## 2. Configuration

### Fichier de Configuration

```toml
# kernel/config/fs.toml

[page_cache]
max_pages = 262144        # 1GB @ 4KB pages
read_ahead_kb = 16        # Read-ahead 16KB
write_back_interval = 30  # Write-back every 30s

[fd_table]
max_fds = 1024            # Max FDs par process

[caches]
dentry_cache_size = 8192
path_cache_size = 8192
symlink_cache_size = 4096

[quotas]
enabled = true
default_soft_blocks = 1048576  # 1GB soft
default_hard_blocks = 2097152  # 2GB hard

[acl]
enabled = true

[inotify]
max_watches = 8192
```

### Chargement Configuration

```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct FsConfig {
    page_cache: PageCacheConfig,
    fd_table: FdTableConfig,
    caches: CachesConfig,
    quotas: QuotaConfig,
    acl: AclConfig,
    inotify: InotifyConfig,
}

fn load_config() -> FsConfig {
    let config_str = include_str!("../../config/fs.toml");
    toml::from_str(config_str).unwrap()
}
```

---

## 3. Syscalls POSIX

### Mapping Syscalls

```rust
// kernel/src/syscall/mod.rs

pub fn handle_syscall(num: usize, args: &[u64]) -> Result<i64, SyscallError> {
    match num {
        // File I/O
        0 => sys_read(args[0] as i32, args[1] as *mut u8, args[2] as usize),
        1 => sys_write(args[0] as i32, args[1] as *const u8, args[2] as usize),
        2 => sys_open(args[0] as *const u8, args[1] as i32, args[2] as u32),
        3 => sys_close(args[0] as i32),
        8 => sys_lseek(args[0] as i32, args[1] as i64, args[2] as i32),
        
        // Vectored I/O
        19 => sys_readv(args[0] as i32, args[1] as *const iovec, args[2] as i32),
        20 => sys_writev(args[0] as i32, args[1] as *const iovec, args[2] as i32),
        
        // Memory mapping
        9 => sys_mmap(args[0] as *mut u8, args[1] as usize, args[2] as i32, 
                      args[3] as i32, args[4] as i32, args[5] as i64),
        11 => sys_munmap(args[0] as *mut u8, args[1] as usize),
        
        // File locking
        72 => sys_fcntl(args[0] as i32, args[1] as i32, args[2] as u64),
        73 => sys_flock(args[0] as i32, args[1] as i32),
        
        // Zero-copy
        40 => sys_sendfile(args[0] as i32, args[1] as i32, 
                           args[2] as *mut i64, args[3] as usize),
        275 => sys_splice(args[0] as i32, args[1] as *mut i64,
                          args[2] as i32, args[3] as *mut i64,
                          args[4] as usize, args[5] as u32),
        
        // io_uring
        425 => sys_io_uring_setup(args[0] as u32, args[1] as *mut IoUringParams),
        426 => sys_io_uring_enter(args[0] as i32, args[1] as u32, 
                                   args[2] as u32, args[3] as u32),
        
        // inotify
        253 => sys_inotify_init(),
        254 => sys_inotify_add_watch(args[0] as i32, args[1] as *const u8, args[2] as u32),
        255 => sys_inotify_rm_watch(args[0] as i32, args[1] as i32),
        
        // Quotas
        179 => sys_quotactl(args[0] as i32, args[1] as *const u8, 
                            args[2] as i32, args[3] as *mut u8),
        
        _ => Err(SyscallError::NotImplemented),
    }
}
```

---

## 4. Debugging

### Logs

```rust
// Activer logs filesystem
export RUST_LOG=exo_os::fs=debug

// Logs détaillés par module
export RUST_LOG=exo_os::fs::vfs=trace,exo_os::fs::ext4=debug
```

### Exemple Logs

```
[DEBUG] fs::vfs: open("/etc/passwd") -> inode 12345
[DEBUG] fs::ext4: read_at(inode=12345, offset=0, len=4096)
[DEBUG] fs::page_cache: cache_hit(inode=12345, page=0)
[DEBUG] fs::vfs: close(fd=3)
```

### Statistiques Runtime

```rust
// API pour obtenir stats
pub fn get_fs_stats() -> FsStats {
    FsStats {
        page_cache_hits: PAGE_CACHE_HITS.load(Ordering::Relaxed),
        page_cache_misses: PAGE_CACHE_MISSES.load(Ordering::Relaxed),
        dentry_cache_hits: DENTRY_CACHE_HITS.load(Ordering::Relaxed),
        total_reads: TOTAL_READS.load(Ordering::Relaxed),
        total_writes: TOTAL_WRITES.load(Ordering::Relaxed),
    }
}

// Afficher stats
println!("{:#?}", get_fs_stats());
```

---

## 5. Troubleshooting

### Problème : FD Table Full

**Symptôme** : `EMFILE` (Too many open files)

**Solution** :
```rust
// Augmenter limite
const MAX_FDS: usize = 4096;  // vs 1024
```

### Problème : Page Cache Full

**Symptôme** : Performance dégradée, beaucoup de disk I/O

**Solution** :
```rust
// Augmenter taille cache
const MAX_PAGES: usize = 524288;  // 2GB @ 4KB pages
```

### Problème : Deadlock File Locks

**Symptôme** : Process bloqué sur `fcntl(F_SETLKW)`

**Debug** :
```rust
// Activer deadlock detector logs
export RUST_LOG=exo_os::fs::locks=debug

// Le deadlock detector détecte automatiquement les cycles
// et retourne EDEADLK après timeout
```

### Problème : Quota Exceeded

**Symptôme** : `EDQUOT` sur write

**Debug** :
```bash
# Vérifier quotas utilisateur
quotactl Q_GETQUOTA /dev/sda1 1000

# Augmenter quota
quotactl Q_SETQUOTA /dev/sda1 1000 soft=2GB hard=4GB
```

---

## 6. Migration depuis Autre OS

### Depuis Linux

**Compatibilité** :
- ✅ 100% POSIX-compliant
- ✅ Same syscall numbers
- ✅ ext4 compatible
- ✅ FAT32 compatible
- ✅ APIs identiques (POSIX, io_uring, inotify, etc.)

**Différences** :
- ⚠️ ext4 journaling simplifié (data=ordered seulement)
- ⚠️ FAT32 lecture seule pour l'instant
- ⚠️ Pas de XFS/Btrfs support

**Migration** :
```bash
# 1. Monter partition ext4 Linux
mount /dev/sda1 /mnt

# 2. Pas de conversion nécessaire !
# 3. Boot Exo-OS avec /dev/sda1 comme root
```

### Depuis Windows

**Compatibilité** :
- ✅ FAT32 lecture complète
- ⚠️ FAT32 écriture en développement
- ❌ NTFS non supporté (pour l'instant)

**Migration** :
```bash
# 1. Formater partition en FAT32 ou ext4
# 2. Copier fichiers
# 3. Boot Exo-OS
```

---

## 7. Tests

### Tests Unitaires

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_open_read_close() {
        // Créer fichier test
        let inode = vfs::create("/tmp/test.txt", InodeType::File).unwrap();
        
        // Écrire
        inode.write_at(0, b"Hello, World!").unwrap();
        
        // Lire
        let mut buf = [0u8; 13];
        let n = inode.read_at(0, &mut buf).unwrap();
        assert_eq!(n, 13);
        assert_eq!(&buf, b"Hello, World!");
        
        // Supprimer
        vfs::remove("/tmp/test.txt").unwrap();
    }
    
    #[test]
    fn test_directory_operations() {
        vfs::mkdir("/tmp/testdir").unwrap();
        vfs::create("/tmp/testdir/file1.txt", InodeType::File).unwrap();
        vfs::create("/tmp/testdir/file2.txt", InodeType::File).unwrap();
        
        let entries = vfs::readdir("/tmp/testdir").unwrap();
        assert_eq!(entries.len(), 2);
        
        vfs::remove("/tmp/testdir/file1.txt").unwrap();
        vfs::remove("/tmp/testdir/file2.txt").unwrap();
        vfs::rmdir("/tmp/testdir").unwrap();
    }
}
```

### Tests d'Intégration

```bash
# Lancer suite de tests
cargo test --package exo-os --lib fs

# Tests spécifiques
cargo test --package exo-os --lib fs::vfs
cargo test --package exo-os --lib fs::ext4
```

---

## 8. Performance Monitoring

### Métriques à Surveiller

```rust
struct FsMetrics {
    // Cache metrics
    page_cache_hit_rate: f64,      // Target: >85%
    dentry_cache_hit_rate: f64,    // Target: >90%
    
    // I/O metrics
    avg_read_latency_ns: u64,      // Target: <500ns
    avg_write_latency_ns: u64,     // Target: <700ns
    
    // Resource metrics
    open_fds: usize,               // Max: MAX_FDS
    dirty_pages: usize,            // Max: MAX_PAGES * 0.3
}

// Obtenir métriques
let metrics = fs::get_metrics();

// Alerter si problèmes
if metrics.page_cache_hit_rate < 0.80 {
    log::warn!("Low page cache hit rate: {:.1}%", metrics.page_cache_hit_rate * 100.0);
}
```

---

Pour plus de détails :
- [ARCHITECTURE.md](./ARCHITECTURE.md) : Architecture technique
- [API.md](./API.md) : APIs complètes
- [PERFORMANCE.md](./PERFORMANCE.md) : Tuning performance
- [EXAMPLES.md](./EXAMPLES.md) : Exemples pratiques
