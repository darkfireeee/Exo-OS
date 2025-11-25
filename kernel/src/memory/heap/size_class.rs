//! Size Class Management
//! 
//! Provides size class utilities for allocator tiers

/// Size class definition
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeClass {
    /// Thread-local cache (≤256 bytes)
    ThreadLocal(usize),
    /// CPU slab (≤4KB)
    CpuSlab(usize),
    /// Buddy allocator (>4KB)
    Buddy(usize),
}

impl SizeClass {
    /// Classify allocation size
    pub fn classify(size: usize) -> Self {
        if size <= 256 {
            Self::ThreadLocal(Self::round_up_thread_local(size))
        } else if size <= 4096 {
            Self::CpuSlab(Self::round_up_cpu_slab(size))
        } else {
            Self::Buddy(Self::round_up_page(size))
        }
    }

    /// Round up to thread-local size class (16, 32, 64, 128, 256)
    fn round_up_thread_local(size: usize) -> usize {
        match size {
            0..=16 => 16,
            17..=32 => 32,
            33..=64 => 64,
            65..=128 => 128,
            129..=256 => 256,
            _ => 256,
        }
    }

    /// Round up to CPU slab size class (512, 1024, 2048, 4096)
    fn round_up_cpu_slab(size: usize) -> usize {
        match size {
            0..=512 => 512,
            513..=1024 => 1024,
            1025..=2048 => 2048,
            2049..=4096 => 4096,
            _ => 4096,
        }
    }

    /// Round up to page boundary
    fn round_up_page(size: usize) -> usize {
        (size + 4095) & !4095
    }

    /// Get actual size for this class
    pub fn size(&self) -> usize {
        match self {
            Self::ThreadLocal(s) => *s,
            Self::CpuSlab(s) => *s,
            Self::Buddy(s) => *s,
        }
    }

    /// Get expected allocation cycles
    pub fn expected_cycles(&self) -> usize {
        match self {
            Self::ThreadLocal(_) => 8,    // ~8 cycles (cache hit)
            Self::CpuSlab(_) => 50,        // ~50 cycles (atomic ops)
            Self::Buddy(_) => 200,         // ~200 cycles (buddy search)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_thread_local() {
        assert_eq!(SizeClass::classify(16), SizeClass::ThreadLocal(16));
        assert_eq!(SizeClass::classify(64), SizeClass::ThreadLocal(64));
        assert_eq!(SizeClass::classify(200), SizeClass::ThreadLocal(256));
    }

    #[test]
    fn test_classify_cpu_slab() {
        assert_eq!(SizeClass::classify(512), SizeClass::CpuSlab(512));
        assert_eq!(SizeClass::classify(1024), SizeClass::CpuSlab(1024));
        assert_eq!(SizeClass::classify(3000), SizeClass::CpuSlab(4096));
    }

    #[test]
    fn test_classify_buddy() {
        assert_eq!(SizeClass::classify(5000), SizeClass::Buddy(8192));
        assert_eq!(SizeClass::classify(10000), SizeClass::Buddy(12288));
    }
}
