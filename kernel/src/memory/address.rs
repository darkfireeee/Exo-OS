use super::{PhysicalAddress, VirtualAddress};

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
