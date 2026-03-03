// ExoFS — Feature flags nightly requis par le module fs/exofs/
//
// Ces flags sont déclarés ici pour documentation et doivent être
// répercutés dans la racine de crate (kernel/src/lib.rs).
//
// | Flag                | Usage dans ExoFS                                    |
// |---------------------|-----------------------------------------------------|
// | allocator_api       | trait Allocator — allocations arène dans io/cache/  |
// | const_size_of_val   | size_of_val() dans des contextes const (core/types) |
// | try_reserve_kind    | OOM-02 : try_reserve() avec détail d'erreur          |
// | atomic_from_mut     | AtomicU64::from_mut() dans epoch/epoch_counter       |

// allocator_api       → kernel/src/lib.rs : #![feature(allocator_api)]
// const_size_of_val   → kernel/src/lib.rs : #![feature(const_size_of_val)]
// try_reserve_kind    → stabilisé en nightly 2024+ (inclus dans try_reserve)
// atomic_from_mut     → kernel/src/lib.rs si requis par epoch/epoch_counter.rs
