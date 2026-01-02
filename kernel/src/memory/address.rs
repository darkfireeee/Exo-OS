//! Memory address types

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub const fn new(addr: usize) -> Self {
        Self(addr)
    }
    
    pub const fn value(&self) -> usize {
        self.0
    }
    
    /// Returns as u64
    pub const fn as_u64(&self) -> u64 {
        self.0 as u64
    }
    
    pub const fn is_page_aligned(&self) -> bool {
        self.0 % 4096 == 0
    }
    
    /// Add offset to physical address
    pub const fn add(&self, offset: u64) -> Self {
        Self(self.0 + offset as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtualAddress(usize);

impl VirtualAddress {
    pub const fn new(addr: usize) -> Self {
        Self(addr)
    }
    
    pub const fn value(&self) -> usize {
        self.0
    }
    
    /// Alias for value() - returns as usize
    pub const fn as_usize(&self) -> usize {
        self.0
    }
    
    /// Returns as u64
    pub const fn as_u64(&self) -> u64 {
        self.0 as u64
    }
    
    /// Create from pointer
    pub fn from_ptr<T>(ptr: *const T) -> Self {
        Self(ptr as usize)
    }
    
    /// Get page offset (lower 12 bits)
    pub const fn page_offset(&self) -> u64 {
        (self.0 & 0xFFF) as u64
    }
    
    /// Get L4 index (bits 39-47)
    pub const fn l4_index(&self) -> usize {
        (self.0 >> 39) & 0x1FF
    }
    
    /// Get L3 index (bits 30-38)
    pub const fn l3_index(&self) -> usize {
        (self.0 >> 30) & 0x1FF
    }
    
    /// Get L2 index (bits 21-29)
    pub const fn l2_index(&self) -> usize {
        (self.0 >> 21) & 0x1FF
    }
    
    /// Get L1 index (bits 12-20)
    pub const fn l1_index(&self) -> usize {
        (self.0 >> 12) & 0x1FF
    }
    
    pub fn is_kernel(&self) -> bool {
        self.0 >= 0xFFFF_8000_0000_0000
    }
    
    pub const fn is_page_aligned(&self) -> bool {
        self.0 % 4096 == 0
    }
}

pub type VirtAddr = VirtualAddress;
pub type PhysAddr = PhysicalAddress;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UserVirtAddr(VirtualAddress);

impl UserVirtAddr {
    pub fn new(addr: usize) -> Self {
        Self(VirtualAddress::new(addr))
    }
    
    pub fn as_usize(&self) -> usize {
        self.0.value()
    }
    
    pub fn as_ptr<T>(&self) -> *const T {
        self.0.value() as *const T
    }
    
    pub fn as_mut_ptr<T>(&self) -> *mut T {
        self.0.value() as *mut T
    }
    
    pub fn is_user(&self) -> bool {
        !self.0.is_kernel()
    }
    
    pub fn add(&self, offset: usize) -> Self {
        Self(VirtualAddress::new(self.0.value() + offset))
    }
}
