//! IPC Descriptors - File descriptor-like handles for IPC channels
//!
//! Each process has a descriptor table tracking open IPC channels

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;
use super::{IpcError, IpcResult};
use super::fusion_ring::FusionRing;

/// IPC descriptor ID (similar to file descriptor)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IpcDescriptor(pub u64);

/// IPC descriptor type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorType {
    /// Fusion ring channel
    Channel,
    /// Shared memory region
    SharedMemory,
    /// Capability handle
    Capability,
}

/// IPC descriptor entry
pub struct DescriptorEntry {
    /// Descriptor ID
    pub id: IpcDescriptor,
    
    /// Descriptor type
    pub desc_type: DescriptorType,
    
    /// Owner process ID
    pub owner_pid: u64,
    
    /// Flags (read, write, etc.)
    pub flags: u32,
    
    /// Reference to underlying object
    pub object: DescriptorObject,
}

/// Underlying IPC object
pub enum DescriptorObject {
    /// Fusion ring channel
    Channel(Arc<FusionRing>),
    
    /// Shared memory region ID
    SharedMemory(u64),
    
    /// Capability token
    Capability(u64),
}

/// Per-process descriptor table
pub struct DescriptorTable {
    /// Process ID
    pid: u64,
    
    /// Map of descriptor ID -> entry
    descriptors: BTreeMap<IpcDescriptor, DescriptorEntry>,
    
    /// Next available descriptor ID
    next_id: u64,
}

impl DescriptorTable {
    /// Create new descriptor table for process
    pub fn new(pid: u64) -> Self {
        Self {
            pid,
            descriptors: BTreeMap::new(),
            next_id: 0,
        }
    }
    
    /// Allocate new descriptor
    pub fn allocate(&mut self, desc_type: DescriptorType, flags: u32, object: DescriptorObject) -> IpcDescriptor {
        let id = IpcDescriptor(self.next_id);
        self.next_id += 1;
        
        let entry = DescriptorEntry {
            id,
            desc_type,
            owner_pid: self.pid,
            flags,
            object,
        };
        
        self.descriptors.insert(id, entry);
        id
    }
    
    /// Get descriptor entry
    pub fn get(&self, id: IpcDescriptor) -> Option<&DescriptorEntry> {
        self.descriptors.get(&id)
    }
    
    /// Get mutable descriptor entry
    pub fn get_mut(&mut self, id: IpcDescriptor) -> Option<&mut DescriptorEntry> {
        self.descriptors.get_mut(&id)
    }
    
    /// Close descriptor
    pub fn close(&mut self, id: IpcDescriptor) -> IpcResult<()> {
        self.descriptors.remove(&id)
            .ok_or(IpcError::NotFound)?;
        Ok(())
    }
    
    /// List all descriptors
    pub fn list(&self) -> impl Iterator<Item = &DescriptorEntry> {
        self.descriptors.values()
    }
    
    /// Count descriptors by type
    pub fn count_by_type(&self, desc_type: DescriptorType) -> usize {
        self.descriptors.values()
            .filter(|e| e.desc_type == desc_type)
            .count()
    }
}

/// Global descriptor registry (per-process tables)
pub struct DescriptorRegistry {
    /// Map of PID -> descriptor table
    tables: BTreeMap<u64, DescriptorTable>,
}

impl DescriptorRegistry {
    pub const fn new() -> Self {
        Self {
            tables: BTreeMap::new(),
        }
    }
    
    /// Create descriptor table for new process
    pub fn create_table(&mut self, pid: u64) -> &mut DescriptorTable {
        self.tables.entry(pid).or_insert_with(|| DescriptorTable::new(pid))
    }
    
    /// Get process descriptor table
    pub fn get_table(&self, pid: u64) -> Option<&DescriptorTable> {
        self.tables.get(&pid)
    }
    
    /// Get mutable process descriptor table
    pub fn get_table_mut(&mut self, pid: u64) -> Option<&mut DescriptorTable> {
        self.tables.get_mut(&pid)
    }
    
    /// Remove process descriptor table (on exit)
    pub fn remove_table(&mut self, pid: u64) -> Option<DescriptorTable> {
        self.tables.remove(&pid)
    }
}

/// Global descriptor registry
static DESCRIPTOR_REGISTRY: Mutex<DescriptorRegistry> = Mutex::new(DescriptorRegistry::new());

/// Initialize descriptor subsystem for process
pub fn init_process(pid: u64) {
    DESCRIPTOR_REGISTRY.lock().create_table(pid);
}

/// Cleanup descriptor table for process
pub fn cleanup_process(pid: u64) {
    DESCRIPTOR_REGISTRY.lock().remove_table(pid);
}

/// Allocate descriptor for process
pub fn allocate_descriptor(pid: u64, desc_type: DescriptorType, flags: u32, object: DescriptorObject) -> IpcResult<IpcDescriptor> {
    let mut registry = DESCRIPTOR_REGISTRY.lock();
    let table = registry.get_table_mut(pid)
        .ok_or(IpcError::NotFound)?;
    Ok(table.allocate(desc_type, flags, object))
}

/// Get descriptor entry
pub fn get_descriptor(pid: u64, id: IpcDescriptor) -> IpcResult<DescriptorType> {
    let registry = DESCRIPTOR_REGISTRY.lock();
    let table = registry.get_table(pid)
        .ok_or(IpcError::NotFound)?;
    let entry = table.get(id)
        .ok_or(IpcError::NotFound)?;
    Ok(entry.desc_type)
}

/// Close descriptor
pub fn close_descriptor(pid: u64, id: IpcDescriptor) -> IpcResult<()> {
    let mut registry = DESCRIPTOR_REGISTRY.lock();
    let table = registry.get_table_mut(pid)
        .ok_or(IpcError::NotFound)?;
    table.close(id)
}
