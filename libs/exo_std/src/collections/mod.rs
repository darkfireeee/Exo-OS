// libs/exo_std/src/collections/mod.rs
//! Collections optimisées pour environnement no_std
//!
//! Toutes les collections sont conçues pour:
//! - Zero-cost abstractions
//! - Contrôle explicite de la mémoire
//! - Performance optimale
//! - Type safety maximale

pub mod ring_buffer;
pub mod ring_buffer_mpsc;
pub mod ring_buffer_mpmc;
pub mod bounded_vec;
pub mod small_vec;
pub mod intrusive_list;
pub mod radix_tree;
pub mod btree_map;
pub mod hash_map;

pub use ring_buffer::RingBuffer;
pub use ring_buffer_mpsc::RingBufferMpsc;
pub use ring_buffer_mpmc::RingBufferMpmc;
pub use bounded_vec::{BoundedVec, CapacityError};
pub use small_vec::SmallVec;
pub use intrusive_list::{IntrusiveList, IntrusiveNode, Iter as IntrusiveIter, IterMut as IntrusiveIterMut, Cursor as IntrusiveCursor, CursorMut as IntrusiveCursorMut};
pub use radix_tree::RadixTree;
pub use btree_map::BTreeMap;
pub use hash_map::HashMap;
