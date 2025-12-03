# 🗻 Mount Points

## Structure

```rust
pub struct Mount {
    /// Point de montage
    pub mountpoint: Arc<Dentry>,
    
    /// Racine du filesystem monté
    pub root: Arc<Dentry>,
    
    /// Filesystem
    pub fs: Arc<FileSystem>,
    
    /// Options de montage
    pub flags: MountFlags,
    
    /// Source (device path)
    pub source: String,
}
```

## Flags de Montage

```rust
bitflags! {
    pub struct MountFlags: u32 {
        const RDONLY    = 0x0001;  // Read-only
        const NOSUID    = 0x0002;  // Ignore setuid bits
        const NODEV     = 0x0004;  // Interdire devices
        const NOEXEC    = 0x0008;  // Interdire exécution
        const SYNC      = 0x0010;  // Écriture synchrone
        const NOATIME   = 0x0020;  // Ne pas màj atime
        const NODIRATIME = 0x0040; // Ne pas màj atime dirs
        const BIND      = 0x1000;  // Bind mount
        const MOVE      = 0x2000;  // Move mount
    }
}
```

## Table des Mounts

```rust
pub struct MountTable {
    /// Mounts actifs
    mounts: Mutex<Vec<Arc<Mount>>>,
    
    /// Index par mountpoint
    by_mountpoint: Mutex<HashMap<u64, Arc<Mount>>>,
}

impl MountTable {
    pub fn mount(
        &self,
        source: &str,
        target: &str,
        fs_type: &str,
        flags: MountFlags,
    ) -> Result<()> {
        // Résoudre le point de montage
        let mountpoint = resolve_path(target)?;
        
        // Créer le filesystem
        let fs = create_filesystem(fs_type, source)?;
        
        // Créer le mount
        let mount = Arc::new(Mount {
            mountpoint: mountpoint.clone(),
            root: fs.root.clone(),
            fs,
            flags,
            source: source.into(),
        });
        
        // Marquer le dentry comme monté
        mountpoint.flags.insert(DentryFlags::MOUNTED);
        
        // Ajouter à la table
        self.mounts.lock().push(mount.clone());
        self.by_mountpoint.lock().insert(mountpoint.inode.unwrap().ino, mount);
        
        Ok(())
    }
    
    pub fn umount(&self, target: &str) -> Result<()> {
        let mountpoint = resolve_path(target)?;
        
        // Trouver le mount
        let mut mounts = self.mounts.lock();
        let pos = mounts.iter()
            .position(|m| Arc::ptr_eq(&m.mountpoint, &mountpoint))
            .ok_or(FsError::NotFound)?;
        
        let mount = mounts.remove(pos);
        
        // Vérifier si busy
        if Arc::strong_count(&mount.root) > 1 {
            mounts.insert(pos, mount);
            return Err(FsError::DirectoryNotEmpty); // EBUSY
        }
        
        // Démarquer le dentry
        mountpoint.flags.remove(DentryFlags::MOUNTED);
        
        // Retirer de l'index
        self.by_mountpoint.lock().remove(&mountpoint.inode.unwrap().ino);
        
        Ok(())
    }
}
```

## Mounts Initiaux

```rust
pub fn init_mounts() -> Result<()> {
    // Root filesystem (tmpfs ou initramfs)
    mount("none", "/", "tmpfs", MountFlags::empty())?;
    
    // Créer les répertoires de base
    mkdir("/dev", 0o755)?;
    mkdir("/proc", 0o555)?;
    mkdir("/sys", 0o555)?;
    mkdir("/tmp", 0o1777)?;
    
    // Monter les pseudo-filesystems
    mount("none", "/dev", "devfs", MountFlags::empty())?;
    mount("none", "/proc", "procfs", MountFlags::RDONLY)?;
    mount("none", "/sys", "sysfs", MountFlags::RDONLY)?;
    
    Ok(())
}
```

## Traversée de Mount

```rust
pub fn cross_mount(dentry: &Arc<Dentry>) -> Arc<Dentry> {
    if dentry.flags.contains(DentryFlags::MOUNTED) {
        // Trouver le mount
        if let Some(mount) = MOUNT_TABLE.by_mountpoint.lock()
            .get(&dentry.inode.as_ref().unwrap().ino) 
        {
            return mount.root.clone();
        }
    }
    dentry.clone()
}
```
