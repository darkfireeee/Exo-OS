//! Production-grade Address Types
//!
//! Physical and virtual address types with full x86-64 support.
//! Includes canonical address checking, page alignment, and conversions.
//!
//! # Safety & Performance
//! - Zero-cost abstractions with `repr(transparent)`
//! - Compile-time validation where possible
//! - Runtime validation with clear error reporting
//! - Optimized for hot paths with `#[inline(always)]`

use core::fmt;
use core::ops::{Add, AddAssign, Sub, SubAssign};

/// Page size (4KB)
pub const PAGE_SIZE: usize = 4096;
/// Page size as u64 (4KB)
pub const PAGE_SIZE_U64: u64 = 4096;

/// Huge page size (2MB)
pub const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;
/// Huge page size as u64 (2MB)
pub const HUGE_PAGE_SIZE_U64: u64 = 2 * 1024 * 1024;

/// Giga page size (1GB - x86-64)
pub const GIGA_PAGE_SIZE: usize = 1024 * 1024 * 1024;
/// Giga page size as u64 (1GB)
pub const GIGA_PAGE_SIZE_U64: u64 = 1024 * 1024 * 1024;

/// Maximum physical address bits (52 bits on modern x86-64)
pub const MAX_PHYS_ADDR_BITS: u8 = 52;

/// Maximum physical address value
pub const MAX_PHYS_ADDR: u64 = (1 << MAX_PHYS_ADDR_BITS) - 1;

/// Physical Address
///
/// Represents a physical memory address with validation and utilities.
/// Uses `repr(transparent)` for zero-cost abstraction.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PhysAddr(u64);

impl PhysAddr {
    /// Create a new physical address with validation
    ///
    /// # Panics
    /// Panics in debug mode if address exceeds physical address space.
    /// In release mode, behavior is undefined for invalid addresses.
    #[inline(always)]
    pub const fn new(addr: u64) -> Self {
        debug_assert!(addr <= MAX_PHYS_ADDR, "Physical address out of range");
        PhysAddr(addr)
    }

    /// Create without validation (unsafe, but const)
    ///
    /// # Safety
    /// Caller must ensure `addr <= MAX_PHYS_ADDR`
    #[inline(always)]
    pub const unsafe fn new_unchecked(addr: u64) -> Self {
        PhysAddr(addr)
    }

    /// Try to create, returning None if invalid
    #[inline(always)]
    pub const fn try_new(addr: u64) -> Option<Self> {
        if addr <= MAX_PHYS_ADDR {
            Some(PhysAddr(addr))
        } else {
            None
        }
    }

    /// Zero address
    pub const ZERO: Self = PhysAddr(0);

    /// Get raw address value
    #[inline(always)]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Get as usize (truncates on 32-bit systems)
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Get as const pointer
    #[inline(always)]
    pub const fn as_ptr<T>(self) -> *const T {
        self.0 as *const T
    }

    /// Get as mutable pointer
    #[inline(always)]
    pub const fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }

    /// Check if page-aligned (4KB)
    #[inline(always)]
    pub const fn is_page_aligned(self) -> bool {
        self.0 & (PAGE_SIZE_U64 - 1) == 0
    }

    /// Check if huge-page-aligned (2MB)
    #[inline(always)]
    pub const fn is_huge_page_aligned(self) -> bool {
        self.0 & (HUGE_PAGE_SIZE_U64 - 1) == 0
    }

    /// Check if giga-page-aligned (1GB)
    #[inline(always)]
    pub const fn is_giga_page_aligned(self) -> bool {
        self.0 & (GIGA_PAGE_SIZE_U64 - 1) == 0
    }

    /// Align down to specified boundary (must be power of 2)
    #[inline(always)]
    pub const fn align_down(self, align: u64) -> Self {
        debug_assert!(align.is_power_of_two(), "Alignment must be power of 2");
        PhysAddr(self.0 & !(align - 1))
    }

    /// Align up to specified boundary (must be power of 2)
    #[inline(always)]
    pub const fn align_up(self, align: u64) -> Self {
        debug_assert!(align.is_power_of_two(), "Alignment must be power of 2");
        PhysAddr((self.0.wrapping_add(align - 1)) & !(align - 1))
    }

    /// Align down to page boundary (4KB)
    #[inline(always)]
    pub const fn page_align_down(self) -> Self {
        PhysAddr(self.0 & !(PAGE_SIZE_U64 - 1))
    }

    /// Align up to page boundary (4KB)
    #[inline(always)]
    pub const fn page_align_up(self) -> Self {
        PhysAddr((self.0 + PAGE_SIZE_U64 - 1) & !(PAGE_SIZE_U64 - 1))
    }

    /// Check if zero
    #[inline(always)]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    /// Checked addition
    #[inline(always)]
    pub const fn checked_add(self, offset: u64) -> Option<Self> {
        match self.0.checked_add(offset) {
            Some(addr) if addr <= MAX_PHYS_ADDR => Some(PhysAddr(addr)),
            _ => None,
        }
    }

    /// Checked subtraction
    #[inline(always)]
    pub const fn checked_sub(self, offset: u64) -> Option<Self> {
        match self.0.checked_sub(offset) {
            Some(addr) => Some(PhysAddr(addr)),
            None => None,
        }
    }

    /// Saturating addition
    #[inline(always)]
    pub const fn saturating_add(self, offset: u64) -> Self {
        let result = self.0.saturating_add(offset);
        if result > MAX_PHYS_ADDR {
            PhysAddr(MAX_PHYS_ADDR)
        } else {
            PhysAddr(result)
        }
    }

    /// Saturating subtraction
    #[inline(always)]
    pub const fn saturating_sub(self, offset: u64) -> Self {
        PhysAddr(self.0.saturating_sub(offset))
    }
}

impl fmt::Debug for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhysAddr")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

impl fmt::Display for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

impl fmt::LowerHex for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)
    }
}

impl fmt::UpperHex for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::UpperHex::fmt(&self.0, f)
    }
}

impl TryFrom<u64> for PhysAddr {
    type Error = ();
    
    #[inline(always)]
    fn try_from(addr: u64) -> Result<Self, Self::Error> {
        Self::try_new(addr).ok_or(())
    }
}

impl TryFrom<usize> for PhysAddr {
    type Error = ();
    
    #[inline(always)]
    fn try_from(addr: usize) -> Result<Self, Self::Error> {
        Self::try_new(addr as u64).ok_or(())
    }
}

impl From<PhysAddr> for u64 {
    #[inline(always)]
    fn from(addr: PhysAddr) -> Self {
        addr.0
    }
}

impl From<PhysAddr> for usize {
    #[inline(always)]
    fn from(addr: PhysAddr) -> Self {
        addr.0 as usize
    }
}

impl Add<u64> for PhysAddr {
    type Output = Self;
    
    #[inline(always)]
    fn add(self, rhs: u64) -> Self {
        PhysAddr(self.0.wrapping_add(rhs))
    }
}

impl Add<usize> for PhysAddr {
    type Output = Self;
    
    #[inline(always)]
    fn add(self, rhs: usize) -> Self {
        PhysAddr(self.0.wrapping_add(rhs as u64))
    }
}

impl AddAssign<u64> for PhysAddr {
    #[inline(always)]
    fn add_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_add(rhs);
    }
}

impl AddAssign<usize> for PhysAddr {
    #[inline(always)]
    fn add_assign(&mut self, rhs: usize) {
        self.0 = self.0.wrapping_add(rhs as u64);
    }
}

impl Sub<u64> for PhysAddr {
    type Output = Self;
    
    #[inline(always)]
    fn sub(self, rhs: u64) -> Self {
        PhysAddr(self.0.wrapping_sub(rhs))
    }
}

impl Sub<usize> for PhysAddr {
    type Output = Self;
    
    #[inline(always)]
    fn sub(self, rhs: usize) -> Self {
        PhysAddr(self.0.wrapping_sub(rhs as u64))
    }
}

impl Sub<PhysAddr> for PhysAddr {
    type Output = u64;
    
    #[inline(always)]
    fn sub(self, rhs: PhysAddr) -> u64 {
        self.0.wrapping_sub(rhs.0)
    }
}

impl SubAssign<u64> for PhysAddr {
    #[inline(always)]
    fn sub_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_sub(rhs);
    }
}

impl SubAssign<usize> for PhysAddr {
    #[inline(always)]
    fn sub_assign(&mut self, rhs: usize) {
        self.0 = self.0.wrapping_sub(rhs as u64);
    }
}

/// Virtual Address
///
/// Represents a virtual (linear) address with canonical address support for x86-64.
/// On x86-64, only 48 bits are used for virtual addresses, and bits 48-63 must be
/// copies of bit 47 (sign-extension).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct VirtAddr(u64);

impl VirtAddr {
    /// Create a new virtual address with canonical check
    ///
    /// # Panics
    /// Panics in debug mode if address is not canonical
    #[inline(always)]
    pub fn new(addr: u64) -> Self {
        debug_assert!(Self::is_canonical(addr), "Non-canonical virtual address: {:#x}", addr);
        VirtAddr(addr)
    }

    /// Create without validation (unsafe)
    ///
    /// # Safety
    /// Caller must ensure address is canonical on x86-64
    #[inline(always)]
    pub const unsafe fn new_unchecked(addr: u64) -> Self {
        VirtAddr(addr)
    }

    /// Try to create, returning None if non-canonical
    #[inline(always)]
    pub fn try_new(addr: u64) -> Option<Self> {
        if Self::is_canonical(addr) {
            Some(VirtAddr(addr))
        } else {
            None
        }
    }

    /// Zero address
    pub const ZERO: Self = VirtAddr(0);

    /// Check if address is canonical (x86-64)
    ///
    /// On x86-64, bits 48-63 must be copies of bit 47
    #[inline(always)]
    pub fn is_canonical(addr: u64) -> bool {
        const CANON_MASK: u64 = 0xFFFF_8000_0000_0000;
        const SIGN_BIT: u64 = 0x0000_8000_0000_0000;
        
        let top_bits = addr & CANON_MASK;
        top_bits == 0 || top_bits == CANON_MASK || (addr & SIGN_BIT == 0 && top_bits == 0)
    }

    /// Make address canonical by sign-extending bit 47
    #[inline(always)]
    pub const fn canonicalize(addr: u64) -> Self {
        const SIGN_BIT: u64 = 0x0000_8000_0000_0000;
        const CANON_MASK: u64 = 0xFFFF_0000_0000_0000;
        
        let canonical = if addr & SIGN_BIT != 0 {
            addr | CANON_MASK
        } else {
            addr & !CANON_MASK
        };
        VirtAddr(canonical)
    }

    /// Get raw address value
    #[inline(always)]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Get as usize
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Get as const pointer
    #[inline(always)]
    pub const fn as_ptr<T>(self) -> *const T {
        self.0 as *const T
    }

    /// Get as mutable pointer
    #[inline(always)]
    pub const fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }

    /// Check if page-aligned (4KB)
    #[inline(always)]
    pub const fn is_page_aligned(self) -> bool {
        self.0 & (PAGE_SIZE_U64 - 1) == 0
    }

    /// Check if huge-page-aligned (2MB)
    #[inline(always)]
    pub const fn is_huge_page_aligned(self) -> bool {
        self.0 & (HUGE_PAGE_SIZE_U64 - 1) == 0
    }

    /// Check if giga-page-aligned (1GB)
    #[inline(always)]
    pub const fn is_giga_page_aligned(self) -> bool {
        self.0 & (GIGA_PAGE_SIZE_U64 - 1) == 0
    }

    /// Align down to specified boundary (must be power of 2)
    /// Returns canonical address
    #[inline(always)]
    pub fn align_down(self, align: u64) -> Self {
        debug_assert!(align.is_power_of_two(), "Alignment must be power of 2");
        let aligned = self.0 & !(align - 1);
        Self::canonicalize(aligned)
    }

    /// Align up to specified boundary (must be power of 2)
    /// Returns canonical address
    #[inline(always)]
    pub fn align_up(self, align: u64) -> Self {
        debug_assert!(align.is_power_of_two(), "Alignment must be power of 2");
        let aligned = (self.0.wrapping_add(align - 1)) & !(align - 1);
        Self::canonicalize(aligned)
    }

    /// Align down to page boundary (4KB)
    #[inline(always)]
    pub const fn page_align_down(self) -> Self {
        VirtAddr(self.0 & !(PAGE_SIZE_U64 - 1))
    }

    /// Align up to page boundary (4KB)
    #[inline(always)]
    pub const fn page_align_up(self) -> Self {
        VirtAddr((self.0 + PAGE_SIZE_U64 - 1) & !(PAGE_SIZE_U64 - 1))
    }

    /// Check if null
    #[inline(always)]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    /// Get page offset (lower 12 bits)
    #[inline(always)]
    pub const fn page_offset(self) -> u64 {
        self.0 & (PAGE_SIZE_U64 - 1)
    }

    /// Get page frame number
    #[inline(always)]
    pub const fn page_number(self) -> u64 {
        self.0 >> 12
    }

    /// Checked addition
    #[inline(always)]
    pub fn checked_add(self, offset: u64) -> Option<Self> {
        let result = self.0.wrapping_add(offset);
        Self::try_new(result)
    }

    /// Checked subtraction
    #[inline(always)]
    pub fn checked_sub(self, offset: u64) -> Option<Self> {
        let result = self.0.wrapping_sub(offset);
        Self::try_new(result)
    }

    /// Saturating addition (returns canonical address)
    #[inline(always)]
    pub fn saturating_add(self, offset: u64) -> Self {
        let result = self.0.saturating_add(offset);
        Self::canonicalize(result)
    }

    /// Saturating subtraction (returns canonical address)
    #[inline(always)]
    pub fn saturating_sub(self, offset: u64) -> Self {
        let result = self.0.saturating_sub(offset);
        Self::canonicalize(result)
    }
}

impl fmt::Debug for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VirtAddr")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

impl fmt::Display for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

impl fmt::LowerHex for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)
    }
}

impl fmt::UpperHex for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::UpperHex::fmt(&self.0, f)
    }
}

impl TryFrom<u64> for VirtAddr {
    type Error = ();
    
    #[inline(always)]
    fn try_from(addr: u64) -> Result<Self, Self::Error> {
        Self::try_new(addr).ok_or(())
    }
}

impl TryFrom<usize> for VirtAddr {
    type Error = ();
    
    #[inline(always)]
    fn try_from(addr: usize) -> Result<Self, Self::Error> {
        Self::try_new(addr as u64).ok_or(())
    }
}

impl<T> From<*const T> for VirtAddr {
    #[inline(always)]
    fn from(ptr: *const T) -> Self {
        VirtAddr(ptr as u64)
    }
}

impl<T> From<*mut T> for VirtAddr {
    #[inline(always)]
    fn from(ptr: *mut T) -> Self {
        VirtAddr(ptr as u64)
    }
}

impl From<VirtAddr> for u64 {
    #[inline(always)]
    fn from(addr: VirtAddr) -> Self {
        addr.0
    }
}

impl From<VirtAddr> for usize {
    #[inline(always)]
    fn from(addr: VirtAddr) -> Self {
        addr.0 as usize
    }
}

impl Add<u64> for VirtAddr {
    type Output = Self;
    
    #[inline(always)]
    fn add(self, rhs: u64) -> Self {
        VirtAddr(self.0.wrapping_add(rhs))
    }
}

impl Add<usize> for VirtAddr {
    type Output = Self;
    
    #[inline(always)]
    fn add(self, rhs: usize) -> Self {
        VirtAddr(self.0.wrapping_add(rhs as u64))
    }
}

impl AddAssign<u64> for VirtAddr {
    #[inline(always)]
    fn add_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_add(rhs);
    }
}

impl AddAssign<usize> for VirtAddr {
    #[inline(always)]
    fn add_assign(&mut self, rhs: usize) {
        self.0 = self.0.wrapping_add(rhs as u64);
    }
}

impl Sub<u64> for VirtAddr {
    type Output = Self;
    
    #[inline(always)]
    fn sub(self, rhs: u64) -> Self {
        VirtAddr(self.0.wrapping_sub(rhs))
    }
}

impl Sub<usize> for VirtAddr {
    type Output = Self;
    
    #[inline(always)]
    fn sub(self, rhs: usize) -> Self {
        VirtAddr(self.0.wrapping_sub(rhs as u64))
    }
}

impl Sub<VirtAddr> for VirtAddr {
    type Output = u64;
    
    #[inline(always)]
    fn sub(self, rhs: VirtAddr) -> u64 {
        self.0.wrapping_sub(rhs.0)
    }
}

impl SubAssign<u64> for VirtAddr {
    #[inline(always)]
    fn sub_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_sub(rhs);
    }
}

impl SubAssign<usize> for VirtAddr {
    #[inline(always)]
    fn sub_assign(&mut self, rhs: usize) {
        self.0 = self.0.wrapping_sub(rhs as u64);
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;
    use std::format;

    // ===== PhysAddr Tests =====

    #[test]
    fn test_phys_addr_new() {
        let addr = PhysAddr::new(0x1000);
        assert_eq!(addr.as_u64(), 0x1000);
        
        let zero = PhysAddr::new(0);
        assert_eq!(zero, PhysAddr::ZERO);
        assert!(zero.is_null());
    }

    #[test]
    fn test_phys_addr_try_new() {
        assert!(PhysAddr::try_new(0).is_some());
        assert!(PhysAddr::try_new(0x1000).is_some());
        assert!(PhysAddr::try_new(MAX_PHYS_ADDR).is_some());
        assert!(PhysAddr::try_new(MAX_PHYS_ADDR + 1).is_none());
        assert!(PhysAddr::try_new(u64::MAX).is_none());
    }

    #[test]
    fn test_phys_addr_alignment() {
        let addr = PhysAddr::new(0x1234);
        assert!(!addr.is_page_aligned());
        assert!(!addr.is_huge_page_aligned());
        
        let page_aligned = PhysAddr::new(0x1000);
        assert!(page_aligned.is_page_aligned());
        assert!(!page_aligned.is_huge_page_aligned());
        
        let huge_aligned = PhysAddr::new(0x20_0000);
        assert!(huge_aligned.is_page_aligned());
        assert!(huge_aligned.is_huge_page_aligned());
        assert!(!huge_aligned.is_giga_page_aligned());
        
        let giga_aligned = PhysAddr::new(0x4000_0000);
        assert!(giga_aligned.is_page_aligned());
        assert!(giga_aligned.is_huge_page_aligned());
        assert!(giga_aligned.is_giga_page_aligned());
    }

    #[test]
    fn test_phys_addr_align_down() {
        let addr = PhysAddr::new(0x1234);
        assert_eq!(addr.align_down(PAGE_SIZE_U64), PhysAddr::new(0x1000));
        assert_eq!(addr.page_align_down(), PhysAddr::new(0x1000));
        
        let addr = PhysAddr::new(0x12_3456);
        assert_eq!(addr.align_down(HUGE_PAGE_SIZE_U64), PhysAddr::new(0x0));
        
        let already_aligned = PhysAddr::new(0x1000);
        assert_eq!(already_aligned.align_down(PAGE_SIZE_U64), already_aligned);
    }

    #[test]
    fn test_phys_addr_align_up() {
        let addr = PhysAddr::new(0x1234);
        assert_eq!(addr.align_up(PAGE_SIZE_U64), PhysAddr::new(0x2000));
        assert_eq!(addr.page_align_up(), PhysAddr::new(0x2000));
        
        let addr = PhysAddr::new(0x12_3456);
        assert_eq!(addr.align_up(HUGE_PAGE_SIZE_U64), PhysAddr::new(0x20_0000));
        
        let already_aligned = PhysAddr::new(0x1000);
        assert_eq!(already_aligned.align_up(PAGE_SIZE_U64), already_aligned);
    }

    #[test]
    fn test_phys_addr_arithmetic() {
        let addr = PhysAddr::new(0x1000);
        
        assert_eq!(addr + 0x100_u64, PhysAddr::new(0x1100));
        assert_eq!(addr + 0x100_usize, PhysAddr::new(0x1100));
        assert_eq!(addr - 0x100_u64, PhysAddr::new(0xf00));
        assert_eq!(addr - 0x100_usize, PhysAddr::new(0xf00));
        
        let addr2 = PhysAddr::new(0x2000);
        assert_eq!(addr2 - addr, 0x1000);
    }

    #[test]
    fn test_phys_addr_arithmetic_assign() {
        let mut addr = PhysAddr::new(0x1000);
        addr += 0x100_u64;
        assert_eq!(addr, PhysAddr::new(0x1100));
        
        addr -= 0x100_u64;
        assert_eq!(addr, PhysAddr::new(0x1000));
        
        addr += 0x100_usize;
        assert_eq!(addr, PhysAddr::new(0x1100));
        
        addr -= 0x100_usize;
        assert_eq!(addr, PhysAddr::new(0x1000));
    }

    #[test]
    fn test_phys_addr_checked_ops() {
        let addr = PhysAddr::new(0x1000);
        
        assert_eq!(addr.checked_add(0x100), Some(PhysAddr::new(0x1100)));
        assert_eq!(addr.checked_sub(0x100), Some(PhysAddr::new(0xf00)));
        assert_eq!(addr.checked_sub(0x2000), None);
        
        let max_addr = PhysAddr::new(MAX_PHYS_ADDR);
        assert_eq!(max_addr.checked_add(1), None);
        assert_eq!(max_addr.checked_add(0), Some(max_addr));
    }

    #[test]
    fn test_phys_addr_saturating_ops() {
        let addr = PhysAddr::new(0x1000);
        
        assert_eq!(addr.saturating_add(0x100), PhysAddr::new(0x1100));
        assert_eq!(addr.saturating_sub(0x100), PhysAddr::new(0xf00));
        assert_eq!(addr.saturating_sub(0x2000), PhysAddr::ZERO);
        
        let max_addr = PhysAddr::new(MAX_PHYS_ADDR);
        assert_eq!(max_addr.saturating_add(1000), max_addr);
    }

    #[test]
    fn test_phys_addr_conversions() {
        let addr = PhysAddr::new(0x1000);
        
        assert_eq!(addr.as_u64(), 0x1000);
        assert_eq!(addr.as_usize(), 0x1000);
        
        let u64_val: u64 = addr.into();
        assert_eq!(u64_val, 0x1000);
        
        let usize_val: usize = addr.into();
        assert_eq!(usize_val, 0x1000);
        
        assert_eq!(PhysAddr::try_from(0x1000_u64).unwrap(), addr);
        assert_eq!(PhysAddr::try_from(0x1000_usize).unwrap(), addr);
        assert!(PhysAddr::try_from(u64::MAX).is_err());
    }

    #[test]
    fn test_phys_addr_pointer_conversions() {
        let addr = PhysAddr::new(0x1000);
        
        let ptr: *const u8 = addr.as_ptr();
        assert_eq!(ptr as u64, 0x1000);
        
        let mut_ptr: *mut u64 = addr.as_mut_ptr();
        assert_eq!(mut_ptr as u64, 0x1000);
    }

    #[test]
    fn test_phys_addr_display() {
        let addr = PhysAddr::new(0x1234);
        let display = format!("{}", addr);
        assert!(display.contains("0x1234"));
        
        let debug = format!("{:?}", addr);
        assert!(debug.contains("PhysAddr"));
        assert!(debug.contains("0x1234"));
        
        let hex_lower = format!("{:x}", addr);
        assert_eq!(hex_lower, "1234");
        
        let hex_upper = format!("{:X}", addr);
        assert_eq!(hex_upper, "1234");
    }

    // ===== VirtAddr Tests =====

    #[test]
    fn test_virt_addr_canonical_boundaries() {
        assert!(VirtAddr::is_canonical(0x0));
        assert!(VirtAddr::is_canonical(0x0000_7fff_ffff_ffff));
        
        assert!(!VirtAddr::is_canonical(0x0000_8000_0000_0000));
        assert!(!VirtAddr::is_canonical(0x0000_8000_0000_0001));
        assert!(!VirtAddr::is_canonical(0x7fff_ffff_ffff_ffff));
        assert!(!VirtAddr::is_canonical(0xffff_7fff_ffff_ffff));
        
        assert!(VirtAddr::is_canonical(0xffff_8000_0000_0000));
        assert!(VirtAddr::is_canonical(0xffff_8000_0000_0001));
        assert!(VirtAddr::is_canonical(0xffff_ffff_ffff_ffff));
    }

    #[test]
    fn test_virt_addr_new() {
        let addr = VirtAddr::new(0x1000);
        assert_eq!(addr.as_u64(), 0x1000);
        
        let zero = VirtAddr::ZERO;
        assert!(zero.is_null());
        assert_eq!(zero.as_u64(), 0);
    }

    #[test]
    fn test_virt_addr_try_new() {
        assert!(VirtAddr::try_new(0x0).is_some());
        assert!(VirtAddr::try_new(0x0000_7fff_ffff_ffff).is_some());
        assert!(VirtAddr::try_new(0xffff_8000_0000_0000).is_some());
        assert!(VirtAddr::try_new(0xffff_ffff_ffff_ffff).is_some());
        
        assert!(VirtAddr::try_new(0x0000_8000_0000_0000).is_none());
        assert!(VirtAddr::try_new(0xffff_7fff_ffff_ffff).is_none());
    }

    #[test]
    fn test_virt_addr_canonicalize() {
        let addr = VirtAddr::canonicalize(0x0000_1234_5678_9abc);
        assert_eq!(addr.as_u64(), 0x0000_1234_5678_9abc);
        assert!(VirtAddr::is_canonical(addr.as_u64()));
        
        let addr_high = VirtAddr::canonicalize(0x0000_8000_0000_0000);
        assert!(VirtAddr::is_canonical(addr_high.as_u64()));
        assert_eq!(addr_high.as_u64() & 0xffff_0000_0000_0000, 0xffff_0000_0000_0000);
    }

    #[test]
    fn test_virt_addr_alignment() {
        let addr = VirtAddr::new(0x1234);
        assert!(!addr.is_page_aligned());
        
        let page_aligned = VirtAddr::new(0x1000);
        assert!(page_aligned.is_page_aligned());
        
        let huge_aligned = VirtAddr::new(0x20_0000);
        assert!(huge_aligned.is_huge_page_aligned());
        
        let giga_aligned = VirtAddr::new(0x4000_0000);
        assert!(giga_aligned.is_giga_page_aligned());
    }

    #[test]
    fn test_virt_addr_align_operations() {
        let addr = VirtAddr::new(0x1234);
        
        let aligned_down = addr.align_down(PAGE_SIZE_U64);
        assert_eq!(aligned_down.as_u64(), 0x1000);
        assert!(VirtAddr::is_canonical(aligned_down.as_u64()));
        
        let aligned_up = addr.align_up(PAGE_SIZE_U64);
        assert_eq!(aligned_up.as_u64(), 0x2000);
        assert!(VirtAddr::is_canonical(aligned_up.as_u64()));
        
        assert_eq!(addr.page_align_down().as_u64(), 0x1000);
        assert_eq!(addr.page_align_up().as_u64(), 0x2000);
    }

    #[test]
    fn test_virt_addr_page_operations() {
        let addr = VirtAddr::new(0x1234);
        assert_eq!(addr.page_offset(), 0x234);
        assert_eq!(addr.page_number(), 0x1);
        
        let addr2 = VirtAddr::new(0x1000);
        assert_eq!(addr2.page_offset(), 0);
        assert_eq!(addr2.page_number(), 0x1);
    }

    #[test]
    fn test_virt_addr_arithmetic() {
        let addr = VirtAddr::new(0x1000);
        
        assert_eq!(addr + 0x100_u64, VirtAddr::new(0x1100));
        assert_eq!(addr + 0x100_usize, VirtAddr::new(0x1100));
        assert_eq!(addr - 0x100_u64, VirtAddr::new(0xf00));
        
        let addr2 = VirtAddr::new(0x2000);
        assert_eq!(addr2 - addr, 0x1000);
    }

    #[test]
    fn test_virt_addr_checked_ops() {
        let addr = VirtAddr::new(0x1000);

        assert_eq!(addr.checked_add(0x100), Some(VirtAddr::new(0x1100)));
        assert_eq!(addr.checked_sub(0x100), Some(VirtAddr::new(0xf00)));
        // Subtracting 0x2000 from 0x1000 wraps to a canonical address in upper half
        assert_eq!(addr.checked_sub(0x2000), Some(VirtAddr::new(0xfffffffffffff000)));

        let high_addr = VirtAddr::new(0xffff_ffff_ffff_f000);
        assert!(high_addr.checked_add(0x1000).is_some());
    }

    #[test]
    fn test_virt_addr_saturating_ops() {
        let addr = VirtAddr::new(0x1000);
        
        let add_result = addr.saturating_add(0x100);
        assert_eq!(add_result.as_u64(), 0x1100);
        assert!(VirtAddr::is_canonical(add_result.as_u64()));
        
        let sub_result = addr.saturating_sub(0x2000);
        assert!(VirtAddr::is_canonical(sub_result.as_u64()));
    }

    #[test]
    fn test_virt_addr_conversions() {
        let addr = VirtAddr::new(0x1000);
        
        assert_eq!(addr.as_u64(), 0x1000);
        assert_eq!(addr.as_usize(), 0x1000);
        
        let u64_val: u64 = addr.into();
        assert_eq!(u64_val, 0x1000);
        
        let usize_val: usize = addr.into();
        assert_eq!(usize_val, 0x1000);
        
        assert_eq!(VirtAddr::try_from(0x1000_u64).unwrap(), addr);
        assert_eq!(VirtAddr::try_from(0x1000_usize).unwrap(), addr);
        assert!(VirtAddr::try_from(0x0000_8000_0000_0000_u64).is_err());
    }

    #[test]
    fn test_virt_addr_from_pointers() {
        let value: u64 = 0x1234;
        let ptr: *const u64 = &value;
        let addr = VirtAddr::from(ptr);
        assert!(VirtAddr::is_canonical(addr.as_u64()));
        
        let mut mut_value: u64 = 0x5678;
        let mut_ptr: *mut u64 = &mut mut_value;
        let addr2 = VirtAddr::from(mut_ptr);
        assert!(VirtAddr::is_canonical(addr2.as_u64()));
    }

    #[test]
    fn test_virt_addr_pointer_conversions() {
        let addr = VirtAddr::new(0x1000);
        
        let ptr: *const u8 = addr.as_ptr();
        assert_eq!(ptr as u64, 0x1000);
        
        let mut_ptr: *mut u64 = addr.as_mut_ptr();
        assert_eq!(mut_ptr as u64, 0x1000);
    }

    #[test]
    fn test_virt_addr_display() {
        let addr = VirtAddr::new(0x1234);
        
        let display = format!("{}", addr);
        assert!(display.contains("0x1234"));
        
        let debug = format!("{:?}", addr);
        assert!(debug.contains("VirtAddr"));
        assert!(debug.contains("0x1234"));
        
        let hex_lower = format!("{:x}", addr);
        assert_eq!(hex_lower, "1234");
        
        let hex_upper = format!("{:X}", addr);
        assert_eq!(hex_upper, "1234");
    }

    #[test]
    fn test_constants() {
        assert_eq!(PAGE_SIZE, 4096);
        assert_eq!(HUGE_PAGE_SIZE, 2 * 1024 * 1024);
        assert_eq!(GIGA_PAGE_SIZE, 1024 * 1024 * 1024);
        assert_eq!(MAX_PHYS_ADDR_BITS, 52);
        assert_eq!(MAX_PHYS_ADDR, (1u64 << 52) - 1);
    }

    #[test]
    fn test_size_and_alignment() {
        use core::mem::{size_of, align_of};
        
        assert_eq!(size_of::<PhysAddr>(), size_of::<u64>());
        assert_eq!(align_of::<PhysAddr>(), align_of::<u64>());
        
        assert_eq!(size_of::<VirtAddr>(), size_of::<u64>());
        assert_eq!(align_of::<VirtAddr>(), align_of::<u64>());
    }

    #[test]
    fn test_ordering() {
        let addr1 = PhysAddr::new(0x1000);
        let addr2 = PhysAddr::new(0x2000);
        let addr3 = PhysAddr::new(0x1000);
        
        assert!(addr1 < addr2);
        assert!(addr2 > addr1);
        assert_eq!(addr1, addr3);
        assert_ne!(addr1, addr2);
        
        let vaddr1 = VirtAddr::new(0x1000);
        let vaddr2 = VirtAddr::new(0x2000);
        assert!(vaddr1 < vaddr2);
    }
}
