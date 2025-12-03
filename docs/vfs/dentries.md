# 📂 Dentries (Directory Entries)

## Structure

```rust
pub struct Dentry {
    /// Nom de l'entrée
    pub name: String,
    
    /// Inode associé (None si négatif)
    pub inode: Option<Arc<Inode>>,
    
    /// Parent
    pub parent: Option<Weak<Dentry>>,
    
    /// Enfants (pour les répertoires)
    pub children: Mutex<BTreeMap<String, Arc<Dentry>>>,
    
    /// Flags
    pub flags: DentryFlags,
    
    /// Filesystem
    pub fs: Weak<FileSystem>,
}
```

## Types de Dentry

```rust
bitflags! {
    pub struct DentryFlags: u32 {
        /// Dentry positif (inode existe)
        const POSITIVE = 0x01;
        /// Dentry négatif (inode n'existe pas)
        const NEGATIVE = 0x02;
        /// Dentry monté
        const MOUNTED  = 0x04;
        /// Racine du filesystem
        const ROOT     = 0x08;
    }
}
```

## Cache de Dentries

```rust
pub struct DentryCache {
    /// Hash table pour lookup rapide
    hash: Mutex<HashMap<(u64, String), Arc<Dentry>>>,
    
    /// LRU pour dentries inactifs
    lru: Mutex<VecDeque<Arc<Dentry>>>,
}
```

## Path Resolution

```rust
pub fn resolve_path(path: &str) -> Result<Arc<Dentry>> {
    let mut current = if path.starts_with('/') {
        ROOT_DENTRY.clone()
    } else {
        current_dir()
    };
    
    for component in path.split('/').filter(|s| !s.is_empty()) {
        match component {
            "." => continue,
            ".." => {
                current = current.parent.upgrade()
                    .unwrap_or(current.clone());
            }
            name => {
                // Check cache first
                if let Some(child) = current.children.lock().get(name) {
                    current = child.clone();
                } else {
                    // Lookup in filesystem
                    let inode = current.inode.as_ref()
                        .ok_or(FsError::NotFound)?
                        .ops.lookup(&current.inode.unwrap(), name)?;
                    
                    let dentry = Arc::new(Dentry::new(name, Some(inode)));
                    current.children.lock().insert(name.into(), dentry.clone());
                    current = dentry;
                }
            }
        }
        
        // Check mount points
        if current.flags.contains(DentryFlags::MOUNTED) {
            current = get_mount_root(&current)?;
        }
    }
    
    Ok(current)
}
```

## Negative Dentries

Les dentries négatifs cachent les lookups échoués:

```rust
pub fn lookup_with_negative_cache(dir: &Dentry, name: &str) -> Result<Arc<Dentry>> {
    let children = dir.children.lock();
    
    if let Some(dentry) = children.get(name) {
        if dentry.flags.contains(DentryFlags::NEGATIVE) {
            return Err(FsError::NotFound);
        }
        return Ok(dentry.clone());
    }
    
    drop(children);
    
    // Lookup in filesystem
    match dir.inode.as_ref().unwrap().ops.lookup(&dir.inode.unwrap(), name) {
        Ok(inode) => {
            let dentry = Arc::new(Dentry::new(name, Some(inode)));
            dir.children.lock().insert(name.into(), dentry.clone());
            Ok(dentry)
        }
        Err(FsError::NotFound) => {
            // Create negative dentry
            let dentry = Arc::new(Dentry::negative(name));
            dir.children.lock().insert(name.into(), dentry);
            Err(FsError::NotFound)
        }
        Err(e) => Err(e)
    }
}
```
