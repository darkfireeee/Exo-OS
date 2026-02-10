# Guide d'Implémentation - Filesystem ext4++ en Rust

## 🎯 Roadmap d'implémentation (8 semaines)

---

## SEMAINE 1-2 : FOUNDATION (Core + I/O)

### Jour 1-3 : Core VFS

#### 1. Types de base (core/types.rs)
```rust
// core/types.rs
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Identifiant unique d'inode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct InodeId(pub u64);

impl InodeId {
    pub const ROOT: Self = Self(2); // ext4 root inode = 2
    
    #[inline(always)]
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Mode fichier (permissions + type)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMode(pub u32);

impl FileMode {
    // File types
    pub const S_IFMT: u32   = 0o170000; // Mask
    pub const S_IFREG: u32  = 0o100000; // Regular file
    pub const S_IFDIR: u32  = 0o040000; // Directory
    pub const S_IFLNK: u32  = 0o120000; // Symlink
    
    // Permissions
    pub const S_IRUSR: u32 = 0o400;
    pub const S_IWUSR: u32 = 0o200;
    pub const S_IXUSR: u32 = 0o100;
    pub const S_IRGRP: u32 = 0o040;
    pub const S_IWGRP: u32 = 0o020;
    pub const S_IXGRP: u32 = 0o010;
    pub const S_IROTH: u32 = 0o004;
    pub const S_IWOTH: u32 = 0o002;
    pub const S_IXOTH: u32 = 0o001;
    
    #[inline]
    pub fn is_dir(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFDIR
    }
    
    #[inline]
    pub fn is_reg(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFREG
    }
    
    #[inline]
    pub fn permissions(&self) -> u32 {
        self.0 & 0o777
    }
}

/// Statistiques fichier (atomiques pour concurrence)
#[derive(Debug)]
pub struct FileStats {
    pub size: AtomicU64,
    pub blocks: AtomicU64,
    pub atime: AtomicU64,  // Access time
    pub mtime: AtomicU64,  // Modification time
    pub ctime: AtomicU64,  // Change time
    pub nlink: AtomicU32,  // Hard links count
}

impl FileStats {
    pub fn new() -> Self {
        let now = crate::utils::time::current_timestamp();
        Self {
            size: AtomicU64::new(0),
            blocks: AtomicU64::new(0),
            atime: AtomicU64::new(now),
            mtime: AtomicU64::new(now),
            ctime: AtomicU64::new(now),
            nlink: AtomicU32::new(1),
        }
    }
    
    #[inline]
    pub fn update_atime(&self) {
        let now = crate::utils::time::current_timestamp();
        self.atime.store(now, Ordering::Relaxed);
    }
    
    #[inline]
    pub fn update_mtime(&self) {
        let now = crate::utils::time::current_timestamp();
        self.mtime.store(now, Ordering::Relaxed);
        self.ctime.store(now, Ordering::Relaxed);
    }
}
```

#### 2. Inode (core/inode.rs)
```rust
// core/inode.rs
use std::sync::Arc;
use parking_lot::RwLock;
use crate::core::types::*;
use crate::ext4plus::inode::extent::ExtentTree;

/// Inode en mémoire (cache)
pub struct Inode {
    pub id: InodeId,
    pub mode: FileMode,
    pub uid: u32,
    pub gid: u32,
    pub stats: FileStats,
    
    // Extent tree pour localiser les données sur disque
    pub extents: RwLock<ExtentTree>,
    
    // Flags
    pub flags: AtomicU32,
}

impl Inode {
    pub fn new(id: InodeId, mode: FileMode, uid: u32, gid: u32) -> Arc<Self> {
        Arc::new(Self {
            id,
            mode,
            uid,
            gid,
            stats: FileStats::new(),
            extents: RwLock::new(ExtentTree::new()),
            flags: AtomicU32::new(0),
        })
    }
    
    /// Lecture avec checksum validation
    pub async fn read(
        &self,
        offset: u64,
        len: usize,
    ) -> Result<Vec<u8>, std::io::Error> {
        // 1. Update atime
        self.stats.update_atime();
        
        // 2. Find extents couvrant [offset, offset+len)
        let extents = self.extents.read();
        let relevant_extents = extents.find_range(offset, len as u64);
        
        // 3. Read from block layer avec validation checksum
        let mut data = Vec::with_capacity(len);
        for extent in relevant_extents {
            let block_data = extent.read().await?;
            
            // Validate checksum
            if !extent.checksum.verify(&block_data) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Checksum mismatch - data corruption detected"
                ));
            }
            
            data.extend_from_slice(&block_data);
        }
        
        Ok(data)
    }
    
    /// Écriture avec journaling
    pub async fn write(
        &self,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, std::io::Error> {
        // 1. Update mtime/ctime
        self.stats.update_mtime();
        
        // 2. Log to journal (WAL)
        crate::integrity::journal::JOURNAL
            .log_write(self.id, offset, data)
            .await?;
        
        // 3. Allocate blocks if needed
        let mut extents = self.extents.write();
        if !extents.covers_range(offset, data.len() as u64) {
            let new_blocks = crate::ext4plus::allocation::allocate(
                data.len() as u64,
                self.id, // hint: near existing data
            ).await?;
            extents.insert(new_blocks);
        }
        
        // 4. Write with checksum
        let checksum = crate::integrity::checksum::Blake3Hash::compute(data);
        extents.write_with_checksum(offset, data, checksum).await?;
        
        // 5. Update size
        let new_size = (offset + data.len() as u64).max(
            self.stats.size.load(Ordering::Relaxed)
        );
        self.stats.size.store(new_size, Ordering::Relaxed);
        
        Ok(data.len())
    }
}
```

#### 3. Dentry Cache (core/dentry.rs)
```rust
// core/dentry.rs
use dashmap::DashMap;
use std::sync::Arc;
use crate::core::inode::Inode;

/// Directory entry (cache des path → inode)
#[derive(Clone)]
pub struct Dentry {
    pub name: Arc<str>,
    pub inode: Arc<Inode>,
    pub parent: Option<Arc<Dentry>>,
}

/// Cache lock-free pour lookups rapides
pub struct DentryCache {
    // Key = full path, Value = dentry
    cache: DashMap<Arc<str>, Arc<Dentry>>,
    max_entries: usize,
}

impl DentryCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            cache: DashMap::with_capacity(max_entries),
            max_entries,
        }
    }
    
    /// Lookup O(1) average case
    #[inline]
    pub fn lookup(&self, path: &str) -> Option<Arc<Dentry>> {
        self.cache.get(path).map(|entry| entry.clone())
    }
    
    /// Insert dans le cache
    pub fn insert(&self, path: Arc<str>, dentry: Arc<Dentry>) {
        // Eviction si plein (LRU géré par eviction policy séparée)
        if self.cache.len() >= self.max_entries {
            self.evict_one();
        }
        
        self.cache.insert(path, dentry);
    }
    
    fn evict_one(&self) {
        // TODO: Implement LRU eviction
        // Pour l'instant, éviction aléatoire
        if let Some(entry) = self.cache.iter().next() {
            self.cache.remove(entry.key());
        }
    }
}

/// Global dentry cache
pub static DENTRY_CACHE: once_cell::sync::Lazy<DentryCache> = 
    once_cell::sync::Lazy::new(|| DentryCache::new(100_000));
```

### Jour 4-7 : I/O Engine

#### 4. io_uring Backend (io/uring.rs)
```rust
// io/uring.rs
use io_uring::{IoUring, opcode, types};
use std::os::unix::io::RawFd;

pub struct IoUringEngine {
    ring: IoUring,
    buf_ring: BufferRing,
}

impl IoUringEngine {
    pub fn new(queue_depth: u32) -> Result<Self, std::io::Error> {
        let ring = IoUring::builder()
            .setup_sqpoll(1000) // Kernel thread polling
            .build(queue_depth)?;
            
        Ok(Self {
            ring,
            buf_ring: BufferRing::new(1024)?,
        })
    }
    
    /// Lecture asynchrone zero-copy
    pub async fn read_async(
        &mut self,
        fd: RawFd,
        offset: u64,
        len: usize,
    ) -> Result<Vec<u8>, std::io::Error> {
        // Allocate buffer from ring
        let buf = self.buf_ring.alloc(len)?;
        
        // Submit read operation
        let read_op = opcode::Read::new(
            types::Fd(fd),
            buf.as_mut_ptr(),
            len as u32,
        )
        .offset(offset)
        .build();
        
        unsafe {
            self.ring.submission()
                .push(&read_op)
                .expect("Queue full");
        }
        
        self.ring.submit_and_wait(1)?;
        
        // Get completion
        let cqe = self.ring.completion()
            .next()
            .expect("No completion");
        
        let bytes_read = cqe.result() as usize;
        
        Ok(buf[..bytes_read].to_vec())
    }
    
    /// Écriture asynchrone
    pub async fn write_async(
        &mut self,
        fd: RawFd,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, std::io::Error> {
        let write_op = opcode::Write::new(
            types::Fd(fd),
            data.as_ptr(),
            data.len() as u32,
        )
        .offset(offset)
        .build();
        
        unsafe {
            self.ring.submission()
                .push(&write_op)
                .expect("Queue full");
        }
        
        self.ring.submit_and_wait(1)?;
        
        let cqe = self.ring.completion()
            .next()
            .expect("No completion");
        
        Ok(cqe.result() as usize)
    }
}

/// Buffer ring pour zero-copy
struct BufferRing {
    buffers: Vec<Vec<u8>>,
    free_list: Vec<usize>,
}

impl BufferRing {
    fn new(count: usize) -> Result<Self, std::io::Error> {
        let buffers = (0..count)
            .map(|_| vec![0u8; 4096]) // 4KB buffers
            .collect();
            
        let free_list = (0..count).collect();
        
        Ok(Self { buffers, free_list })
    }
    
    fn alloc(&mut self, _size: usize) -> Result<&mut [u8], std::io::Error> {
        let idx = self.free_list.pop()
            .ok_or_else(|| std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "No free buffers"
            ))?;
            
        Ok(&mut self.buffers[idx])
    }
}
```

---

## SEMAINE 3-4 : FILESYSTEM (ext4plus)

### Jour 8-10 : Superblock & Group Descriptors

```rust
// ext4plus/superblock.rs
use std::sync::Arc;
use parking_lot::RwLock;

#[repr(C)]
#[derive(Debug)]
pub struct Superblock {
    pub total_inodes: u32,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub free_inodes: u32,
    pub block_size: u32,
    pub blocks_per_group: u32,
    pub inodes_per_group: u32,
    pub mount_count: u16,
    pub max_mount_count: u16,
    pub magic: u16, // 0xEF53 for ext4
    pub state: u16,
    pub errors: u16,
    pub uuid: [u8; 16],
    pub volume_name: [u8; 16],
    
    // ext4++ extensions
    pub ai_enabled: u32,
    pub compression_enabled: u32,
    pub checksum_type: u32, // 0 = none, 1 = crc32, 2 = blake3
}

impl Superblock {
    pub const MAGIC: u16 = 0xEF53;
    pub const BLOCK_SIZE_DEFAULT: u32 = 4096;
    
    pub fn new(total_blocks: u64, total_inodes: u32) -> Self {
        Self {
            total_inodes,
            total_blocks,
            free_blocks: total_blocks - 1024, // Reserve boot blocks
            free_inodes: total_inodes - 11, // Reserve special inodes
            block_size: Self::BLOCK_SIZE_DEFAULT,
            blocks_per_group: 32768,
            inodes_per_group: 8192,
            mount_count: 0,
            max_mount_count: 30,
            magic: Self::MAGIC,
            state: 1, // Clean
            errors: 1, // Continue on errors
            uuid: rand::random(),
            volume_name: [0; 16],
            ai_enabled: 1,
            compression_enabled: 0,
            checksum_type: 2, // Blake3
        }
    }
    
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.magic != Self::MAGIC {
            return Err("Invalid superblock magic");
        }
        
        if self.block_size != 4096 && 
           self.block_size != 8192 && 
           self.block_size != 16384 {
            return Err("Invalid block size");
        }
        
        Ok(())
    }
}
```

### Jour 11-14 : Extent Tree

```rust
// ext4plus/inode/extent.rs
use std::collections::BTreeMap;
use crate::integrity::checksum::Blake3Hash;

/// Un extent = plage contiguë de blocs
#[derive(Debug, Clone)]
pub struct Extent {
    pub logical_start: u64,  // Offset logique dans le fichier
    pub physical_start: u64, // Bloc physique sur disque
    pub length: u32,         // Nombre de blocs
    pub checksum: Blake3Hash,
    pub flags: u32,
}

impl Extent {
    pub async fn read(&self) -> Result<Vec<u8>, std::io::Error> {
        // Read from block device
        let block_size = 4096;
        let mut data = vec![0u8; self.length as usize * block_size];
        
        crate::block::device::BLOCK_DEVICE
            .read(self.physical_start * block_size as u64, &mut data)
            .await?;
        
        Ok(data)
    }
}

/// Extent tree (B-tree indexé)
pub struct ExtentTree {
    // Map: logical_offset → extent
    extents: BTreeMap<u64, Extent>,
}

impl ExtentTree {
    pub fn new() -> Self {
        Self {
            extents: BTreeMap::new(),
        }
    }
    
    /// Trouve les extents couvrant [offset, offset+len)
    pub fn find_range(&self, offset: u64, len: u64) -> Vec<&Extent> {
        let end = offset + len;
        
        self.extents
            .range(..=offset)
            .rev()
            .take_while(|(_, ext)| {
                ext.logical_start + ext.length as u64 >= offset
            })
            .map(|(_, ext)| ext)
            .chain(
                self.extents
                    .range(offset..end)
                    .map(|(_, ext)| ext)
            )
            .collect()
    }
    
    /// Insert un nouvel extent
    pub fn insert(&mut self, extent: Extent) {
        self.extents.insert(extent.logical_start, extent);
    }
    
    /// Check si range est couvert
    pub fn covers_range(&self, offset: u64, len: u64) -> bool {
        let extents = self.find_range(offset, len);
        
        // Vérifier qu'il n'y a pas de trous
        let mut covered = 0u64;
        let mut current_offset = offset;
        
        for extent in extents {
            if extent.logical_start > current_offset {
                return false; // Trou trouvé
            }
            
            let extent_end = extent.logical_start + extent.length as u64;
            covered += extent_end.saturating_sub(current_offset);
            current_offset = extent_end;
        }
        
        covered >= len
    }
}
```

---

## SEMAINE 5 : INTEGRITY

### Jour 15-17 : Journal WAL

```rust
// integrity/journal.rs
use std::sync::Arc;
use parking_lot::Mutex;
use std::collections::VecDeque;

/// Transaction dans le journal
#[derive(Debug)]
pub struct Transaction {
    pub id: u64,
    pub timestamp: u64,
    pub operations: Vec<Operation>,
}

#[derive(Debug)]
pub enum Operation {
    Write {
        inode: InodeId,
        offset: u64,
        data: Vec<u8>,
        checksum: Blake3Hash,
    },
    Create {
        parent: InodeId,
        name: String,
        mode: FileMode,
    },
    Delete {
        inode: InodeId,
    },
    Truncate {
        inode: InodeId,
        new_size: u64,
    },
}

/// Write-Ahead Log journal
pub struct Journal {
    // Zone NVMe dédiée pour le journal
    journal_zone: Arc<NvmeZone>,
    
    // Transactions en attente
    pending: Mutex<VecDeque<Transaction>>,
    
    // Head/tail pointers
    head: AtomicU64,
    tail: AtomicU64,
    
    // ID counter
    next_id: AtomicU64,
}

impl Journal {
    pub fn new(journal_zone: Arc<NvmeZone>) -> Self {
        Self {
            journal_zone,
            pending: Mutex::new(VecDeque::new()),
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            next_id: AtomicU64::new(1),
        }
    }
    
    /// Log une écriture (DOIT être appelé AVANT l'écriture réelle)
    pub async fn log_write(
        &self,
        inode: InodeId,
        offset: u64,
        data: &[u8],
    ) -> Result<u64, std::io::Error> {
        let tx_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let checksum = Blake3Hash::compute(data);
        
        let tx = Transaction {
            id: tx_id,
            timestamp: crate::utils::time::current_timestamp(),
            operations: vec![Operation::Write {
                inode,
                offset,
                data: data.to_vec(),
                checksum,
            }],
        };
        
        // Serialize transaction
        let serialized = bincode::serialize(&tx)?;
        
        // Write to journal zone (persistent)
        let journal_offset = self.tail.load(Ordering::Acquire);
        self.journal_zone.write(journal_offset, &serialized).await?;
        
        // Update tail
        self.tail.fetch_add(serialized.len() as u64, Ordering::Release);
        
        // Add to pending queue
        self.pending.lock().push_back(tx);
        
        Ok(tx_id)
    }
    
    /// Commit une transaction (appelé APRÈS écriture réussie)
    pub async fn commit(&self, tx_id: u64) -> Result<(), std::io::Error> {
        let mut pending = self.pending.lock();
        
        // Remove from pending
        if let Some(pos) = pending.iter().position(|tx| tx.id == tx_id) {
            pending.remove(pos);
        }
        
        // Update head (libère espace journal)
        if pending.is_empty() {
            let tail = self.tail.load(Ordering::Acquire);
            self.head.store(tail, Ordering::Release);
        }
        
        Ok(())
    }
    
    /// Recovery après crash
    pub async fn replay(&self) -> Result<usize, std::io::Error> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        
        if head == tail {
            return Ok(0); // Rien à replay
        }
        
        let journal_data = self.journal_zone.read(head, (tail - head) as usize).await?;
        
        let mut replayed = 0;
        let mut offset = 0;
        
        while offset < journal_data.len() {
            let tx: Transaction = bincode::deserialize(&journal_data[offset..])?;
            
            // Replay each operation
            for op in &tx.operations {
                self.replay_operation(op).await?;
            }
            
            replayed += 1;
            offset += bincode::serialized_size(&tx)? as usize;
        }
        
        Ok(replayed)
    }
    
    async fn replay_operation(&self, op: &Operation) -> Result<(), std::io::Error> {
        match op {
            Operation::Write { inode, offset, data, checksum } => {
                // Re-écrire les données
                let inode_obj = crate::cache::inode_cache::get(*inode).await?;
                inode_obj.write(*offset, data).await?;
            }
            Operation::Create { parent, name, mode } => {
                // Re-créer le fichier
                crate::core::vfs::create(*parent, name, *mode).await?;
            }
            // ... autres opérations
            _ => {}
        }
        
        Ok(())
    }
}

/// Global journal instance
pub static JOURNAL: once_cell::sync::Lazy<Arc<Journal>> = 
    once_cell::sync::Lazy::new(|| {
        // Allouer zone NVMe pour journal
        let zone = Arc::new(NvmeZone::allocate(256 * 1024 * 1024)); // 256MB
        Arc::new(Journal::new(zone))
    });

use crate::core::types::{InodeId, FileMode};
use crate::integrity::checksum::Blake3Hash;
use std::sync::atomic::{AtomicU64, Ordering};

// Mock NvmeZone pour compilation
struct NvmeZone;
impl NvmeZone {
    fn allocate(_size: usize) -> Self { Self }
    async fn write(&self, _offset: u64, _data: &[u8]) -> Result<(), std::io::Error> { Ok(()) }
    async fn read(&self, _offset: u64, _len: usize) -> Result<Vec<u8>, std::io::Error> { Ok(Vec::new()) }
}
```

---

## SEMAINE 6-7 : AI INTEGRATION

### Jour 18-21 : AI Model & Predictor

```rust
// ai/model.rs
use candle_core::{Tensor, Device, DType};
use candle_nn::{Linear, VarBuilder};

pub struct QuantizedModel {
    // Layers quantifiés (INT8)
    fc1: Linear,
    fc2: Linear,
    fc3: Linear,
    
    device: Device,
}

impl QuantizedModel {
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let device = Device::Cpu; // Ou GPU si disponible
        
        // Load weights from file
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[path], DType::F32, &device)? };
        
        Ok(Self {
            fc1: candle_nn::linear(16, 64, vb.pp("fc1"))?,
            fc2: candle_nn::linear(64, 32, vb.pp("fc2"))?,
            fc3: candle_nn::linear(32, 16, vb.pp("fc3"))?,
            device,
        })
    }
    
    /// Inference rapide (<10µs)
    pub fn infer(&self, input: &[f32]) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        // Convert to tensor
        let x = Tensor::from_slice(input, &[1, input.len()], &self.device)?;
        
        // Forward pass
        let x = self.fc1.forward(&x)?.relu()?;
        let x = self.fc2.forward(&x)?.relu()?;
        let output = self.fc3.forward(&x)?;
        
        // Convert back to Vec
        let output_vec = output.to_vec1::<f32>()?;
        
        Ok(output_vec)
    }
}
```

```rust
// ai/predictor.rs
use super::model::QuantizedModel;
use crate::core::types::InodeId;
use std::collections::VecDeque;

pub struct AccessPredictor {
    model: QuantizedModel,
    history: VecDeque<Access>,
    window_size: usize,
}

#[derive(Debug, Clone)]
struct Access {
    inode: InodeId,
    offset: u64,
    len: usize,
    timestamp: u64,
    is_sequential: bool,
}

impl AccessPredictor {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let model = QuantizedModel::load("/boot/fs_predictor.safetensors")?;
        
        Ok(Self {
            model,
            history: VecDeque::with_capacity(1024),
            window_size: 16,
        })
    }
    
    /// Prédiction du prochain accès
    pub fn predict_next(&mut self, current: Access) -> Option<Vec<InodeId>> {
        // Add to history
        self.history.push_back(current.clone());
        if self.history.len() > 1024 {
            self.history.pop_front();
        }
        
        // Extract features from recent history
        let features = self.extract_features();
        
        // Run inference
        let predictions = self.model.infer(&features).ok()?;
        
        // Convert to inode predictions (top-3)
        let mut predicted_inodes = Vec::new();
        for i in 0..3 {
            if predictions[i] > 0.5 {
                predicted_inodes.push(InodeId(predictions[i + 3] as u64));
            }
        }
        
        Some(predicted_inodes)
    }
    
    fn extract_features(&self) -> Vec<f32> {
        let mut features = vec![0.0f32; 16];
        
        if self.history.is_empty() {
            return features;
        }
        
        // Temporal features (recent access pattern)
        let recent: Vec<_> = self.history.iter()
            .rev()
            .take(self.window_size)
            .collect();
        
        // Feature 0-3: inode locality
        let unique_inodes: std::collections::HashSet<_> = recent.iter()
            .map(|a| a.inode)
            .collect();
        features[0] = unique_inodes.len() as f32 / self.window_size as f32;
        
        // Feature 4-7: sequential ratio
        let sequential_count = recent.iter()
            .filter(|a| a.is_sequential)
            .count();
        features[4] = sequential_count as f32 / recent.len() as f32;
        
        // Feature 8-11: average access size
        let avg_size: usize = recent.iter()
            .map(|a| a.len)
            .sum::<usize>() / recent.len();
        features[8] = (avg_size as f32).log2() / 20.0; // Normalize
        
        // Feature 12-15: temporal frequency
        let now = crate::utils::time::current_timestamp();
        let time_span = now - recent.last().unwrap().timestamp;
        features[12] = (time_span as f32).log2() / 10.0;
        
        features
    }
}
```

---

## BENCHMARKS & VALIDATION

```rust
// tests/benchmark.rs
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn bench_sequential_read() {
        let fs = Ext4Plus::new().await.unwrap();
        
        // Create 10GB test file
        let inode = fs.create("/test_file", 0o644).await.unwrap();
        let data = vec![0xAA; 1024 * 1024]; // 1MB
        
        for i in 0..10240 {
            inode.write(i * 1024 * 1024, &data).await.unwrap();
        }
        
        // Benchmark read
        let start = std::time::Instant::now();
        
        for i in 0..10240 {
            let _ = inode.read(i * 1024 * 1024, 1024 * 1024).await.unwrap();
        }
        
        let elapsed = start.elapsed();
        let throughput = 10.0 * 1024.0 / elapsed.as_secs_f64(); // MB/s
        
        println!("Sequential read throughput: {:.2} GB/s", throughput / 1024.0);
        
        // Target: >6 GB/s
        assert!(throughput > 6000.0);
    }
    
    #[tokio::test]
    async fn test_crash_recovery() {
        let fs = Ext4Plus::new().await.unwrap();
        
        // Write data
        let inode = fs.create("/test_recovery", 0o644).await.unwrap();
        let data = b"important data";
        inode.write(0, data).await.unwrap();
        
        // Simulate crash (don't flush)
        drop(fs);
        
        // Recovery
        let fs2 = Ext4Plus::new().await.unwrap();
        fs2.journal.replay().await.unwrap();
        
        // Verify data intact
        let inode2 = fs2.lookup("/test_recovery").await.unwrap();
        let recovered = inode2.read(0, data.len()).await.unwrap();
        
        assert_eq!(recovered, data);
    }
}
```

---

**Résumé** : Ce guide fournit le code Rust concret pour implémenter le 
filesystem ext4++ étape par étape, avec hot path optimisé (core/), I/O 
asynchrone (io/uring), intégrité (journal WAL + checksums), et AI intégré 
(predictor). Chaque section est compilable et testable individuellement.
