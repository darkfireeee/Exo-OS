//! NUMA (Non-Uniform Memory Access) support
//! 
//! Provides NUMA-aware memory allocation

use crate::memory::PhysicalAddress;
use super::Frame;

/// NUMA node ID
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumaNode(pub u32);

impl NumaNode {
    pub const fn new(id: u32) -> Self {
        Self(id)
    }
    
    pub const fn id(&self) -> u32 {
        self.0
    }
}

/// NUMA node information
#[derive(Debug)]
pub struct NumaNodeInfo {
    pub node: NumaNode,
    pub base_addr: PhysicalAddress,
    pub size: usize,
    pub free_frames: usize,
}

impl NumaNodeInfo {
    pub fn new(node: NumaNode, base_addr: PhysicalAddress, size: usize) -> Self {
        Self {
            node,
            base_addr,
            size,
            free_frames: size / super::FRAME_SIZE,
        }
    }
}

use alloc::vec::Vec;
use spin::Mutex;

static NUMA_NODES: Mutex<Vec<NumaNodeInfo>> = Mutex::new(Vec::new());

/// Initialize NUMA subsystem
pub fn init() {
    let mut nodes = NUMA_NODES.lock();
    
    // Detect NUMA topology (stub - would parse ACPI SRAT)
    // For now, create single node for UMA systems
    let node_info = NumaNodeInfo::new(
        NumaNode::new(0),
        PhysicalAddress::new(0x100000), // 1MB
        512 * 1024 * 1024, // 512MB
    );
    
    nodes.push(node_info);
    log::info!("NUMA: initialized with {} node(s)", nodes.len());
}

/// Get NUMA node count
pub fn node_count() -> usize {
    NUMA_NODES.lock().len()
}

/// Get NUMA node info
pub fn get_node_info(node: NumaNode) -> Option<NumaNodeInfo> {
    NUMA_NODES.lock()
        .iter()
        .find(|n| n.node == node)
        .cloned()
}

use core::clone::Clone;
impl Clone for NumaNodeInfo {
    fn clone(&self) -> Self {
        Self {
            node: self.node,
            base_addr: self.base_addr,
            size: self.size,
            free_frames: self.free_frames,
        }
    }
}

/// NUMA allocator
pub struct NumaAllocator;

impl NumaAllocator {
    pub fn new() -> Self {
        Self
    }
    
    /// Allocate frame from specific NUMA node
    pub fn allocate_from_node(&mut self, node: NumaNode) -> Option<Frame> {
        let mut nodes = NUMA_NODES.lock();
        
        // Find node
        let node_info = nodes.iter_mut()
            .find(|n| n.node == node)?;
        
        // Check if frames available
        if node_info.free_frames == 0 {
            return None;
        }
        
        // Allocate frame from node's memory range
        let frame_addr = node_info.base_addr.value() + 
            (node_info.size - node_info.free_frames * super::FRAME_SIZE);
        
        node_info.free_frames -= 1;
        
        Some(Frame::new(
            PhysicalAddress::new(frame_addr)
        ))
    }
    
    /// Get closest NUMA node for CPU
    pub fn closest_node(cpu_id: usize) -> NumaNode {
        // Simplified: map CPUs to nodes in round-robin
        let nodes = NUMA_NODES.lock();
        let node_count = nodes.len();
        
        if node_count == 0 {
            return NumaNode::new(0);
        }
        
        NumaNode::new((cpu_id % node_count) as u32)
    }
    
    /// Free frame to node
    pub fn free_to_node(&mut self, _frame: Frame, node: NumaNode) {
        let mut nodes = NUMA_NODES.lock();
        
        if let Some(node_info) = nodes.iter_mut().find(|n| n.node == node) {
            node_info.free_frames += 1;
        }
    }
}

impl Default for NumaAllocator {
    fn default() -> Self {
        Self::new()
    }
}
