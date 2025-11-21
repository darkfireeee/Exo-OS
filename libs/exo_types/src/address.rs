// libs/exo_types/src/address.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PhysAddr(u64);

impl PhysAddr {
    pub const fn new(addr: u64) -> Self {
        PhysAddr(addr)
    }

    pub const fn as_u64(&self) -> u64 {
        self.0
    }
}

impl From<u64> for PhysAddr {
    fn from(addr: u64) -> Self {
        PhysAddr(addr)
    }
}

impl From<PhysAddr> for u64 {
    fn from(addr: PhysAddr) -> Self {
        addr.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct VirtAddr(u64);

impl VirtAddr {
    pub const fn new(addr: u64) -> Self {
        VirtAddr(addr)
    }

    pub const fn as_u64(&self) -> u64 {
        self.0
    }
}

impl From<u64> for VirtAddr {
    fn from(addr: u64) -> Self {
        VirtAddr(addr)
    }
}

impl From<VirtAddr> for u64 {
    fn from(addr: VirtAddr) -> Self {
        addr.0
    }
}
