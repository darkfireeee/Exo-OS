# 📄 Inodes

## Structure

```rust
pub struct Inode {
    /// Numéro d'inode unique
    pub ino: u64,
    
    /// Type (fichier, répertoire, lien, etc.)
    pub inode_type: InodeType,
    
    /// Permissions
    pub mode: u32,
    
    /// Propriétaire
    pub uid: u32,
    pub gid: u32,
    
    /// Taille en octets
    pub size: u64,
    
    /// Timestamps
    pub atime: u64,  // Dernier accès
    pub mtime: u64,  // Dernière modification
    pub ctime: u64,  // Dernier changement de métadonnées
    
    /// Nombre de hard links
    pub nlink: u32,
    
    /// Opérations spécifiques au FS
    pub ops: &'static dyn InodeOps,
    
    /// Données privées du FS
    pub private: *mut c_void,
}
```

## Types d'Inode

```rust
pub enum InodeType {
    Regular,     // Fichier normal
    Directory,   // Répertoire
    Symlink,     // Lien symbolique
    CharDevice,  // Device caractère
    BlockDevice, // Device bloc
    Fifo,        // Named pipe
    Socket,      // Socket Unix
}
```

## Opérations

```rust
pub trait InodeOps: Send + Sync {
    /// Lire des données
    fn read(&self, inode: &Inode, buf: &mut [u8], offset: u64) -> Result<usize>;
    
    /// Écrire des données
    fn write(&self, inode: &Inode, buf: &[u8], offset: u64) -> Result<usize>;
    
    /// Lookup un nom dans un répertoire
    fn lookup(&self, dir: &Inode, name: &str) -> Result<Inode>;
    
    /// Créer un fichier
    fn create(&self, dir: &Inode, name: &str, mode: u32) -> Result<Inode>;
    
    /// Créer un répertoire
    fn mkdir(&self, dir: &Inode, name: &str, mode: u32) -> Result<Inode>;
    
    /// Supprimer un fichier
    fn unlink(&self, dir: &Inode, name: &str) -> Result<()>;
    
    /// Supprimer un répertoire
    fn rmdir(&self, dir: &Inode, name: &str) -> Result<()>;
    
    /// Lire les entrées d'un répertoire
    fn readdir(&self, dir: &Inode) -> Result<Vec<DirEntry>>;
    
    /// Obtenir les attributs
    fn getattr(&self, inode: &Inode) -> Result<InodeAttr>;
    
    /// Modifier les attributs
    fn setattr(&self, inode: &Inode, attr: &InodeAttr) -> Result<()>;
}
```

## Cache d'Inodes

```rust
pub struct InodeCache {
    /// Inodes en cache (ino -> Inode)
    cache: Mutex<BTreeMap<u64, Arc<Inode>>>,
    
    /// LRU pour éviction
    lru: Mutex<VecDeque<u64>>,
    
    /// Taille max du cache
    max_size: usize,
}

impl InodeCache {
    pub fn get(&self, ino: u64) -> Option<Arc<Inode>> {
        let cache = self.cache.lock();
        cache.get(&ino).cloned()
    }
    
    pub fn insert(&self, inode: Arc<Inode>) {
        let mut cache = self.cache.lock();
        let mut lru = self.lru.lock();
        
        // Éviction si nécessaire
        while cache.len() >= self.max_size {
            if let Some(old_ino) = lru.pop_front() {
                cache.remove(&old_ino);
            }
        }
        
        cache.insert(inode.ino, inode.clone());
        lru.push_back(inode.ino);
    }
}
```
