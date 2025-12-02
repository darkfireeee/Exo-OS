//! LRU Cache
//!
//! Least Recently Used cache for permission lookups.
//! Fixed size, O(1) access.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub struct LruCache<K, V> {
    capacity: usize,
    map: BTreeMap<K, V>, // In no_std, we might not have HashMap easily without hashbrown
    // For true O(1) LRU we need a linked list + map.
    // BTreeMap is O(log N).
    // This is a simplified implementation.
    order: Vec<K>,
}

impl<K: Clone + Ord + PartialEq, V> LruCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: BTreeMap::new(),
            order: Vec::with_capacity(capacity),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        if self.map.contains_key(key) {
            // Update order
            if let Some(pos) = self.order.iter().position(|k| k == key) {
                let k = self.order.remove(pos);
                self.order.push(k);
            }
            self.map.get(key)
        } else {
            None
        }
    }

    pub fn put(&mut self, key: K, value: V) {
        if self.map.contains_key(&key) {
            // Update existing
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
            self.order.push(key.clone());
            self.map.insert(key, value);
        } else {
            // Insert new
            if self.map.len() >= self.capacity {
                // Evict LRU
                if !self.order.is_empty() {
                    let lru = self.order.remove(0);
                    self.map.remove(&lru);
                }
            }
            self.order.push(key.clone());
            self.map.insert(key, value);
        }
    }
}
