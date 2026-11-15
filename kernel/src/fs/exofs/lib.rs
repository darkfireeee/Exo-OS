// ExoFS — Nightly feature flags required by module fs/exofs/
//
// These flags are declared here for documentation and must be
// reflected in crate root (kernel/src/lib.rs).
//
// | Flag                | Usage in ExoFS                                      |
// |---------------------|-----------------------------------------------------|
// | allocator_api       | trait Allocator — arena allocations in io/cache/    |
// | const_size_of_val   | size_of_val() in const contexts (core/types)        |
// | try_reserve_kind    | OOM-02: try_reserve() with error details            |
// | atomic_from_mut     | AtomicU64::from_mut() in epoch/epoch_counter         |

// allocator_api       -> kernel/src/lib.rs : #![feature(allocator_api)]
// const_size_of_val   -> kernel/src/lib.rs : #![feature(const_size_of_val)]
// try_reserve_kind    -> stabilized in nightly 2024+ (included in try_reserve)
// atomic_from_mut     -> kernel/src/lib.rs if required by epoch/epoch_counter.rs
