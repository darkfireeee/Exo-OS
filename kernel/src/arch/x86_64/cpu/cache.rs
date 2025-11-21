//! CPU Cache information and management

/// Get the cache line size for the current CPU
pub fn get_cache_line_size() -> usize {
    // Default to 64 bytes for most modern x86-64 CPUs
    64
}

/// Get L1 data cache size
pub fn get_l1d_size() -> usize {
    // Default: 32KB
    32 * 1024
}

/// Get L2 cache size
pub fn get_l2_size() -> usize {
    // Default: 256KB
    256 * 1024
}

/// Get L3 cache size
pub fn get_l3_size() -> usize {
    // Default: 8MB
    8 * 1024 * 1024
}
