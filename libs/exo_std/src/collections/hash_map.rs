//! HashMap robuste no_std avec Robin Hood hashing
//!
//! Implémentation haute-performance utilisant:
//! - Robin Hood hashing pour variance minimale
//! - Linear probing avec distances
//! - FNV-1a hasher rapide et non-cryptographique
//! - Résizing automatique avec facteur de charge 0.75

extern crate alloc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use core::borrow::Borrow;
use core::hash::{Hash, Hasher};
use core::mem;

/// Capacité initiale (doit être puissance de 2)
const INITIAL_CAPACITY: usize = 16;
/// Facteur de charge maximum avant resize
const LOAD_FACTOR: f32 = 0.75;

/// Simple hasher FNV-1a (rapide, non-cryptographique)
struct FnvHasher {
    state: u64,
}

impl FnvHasher {
    #[inline]
    const fn new() -> Self {
        Self {
            state: 0xcbf29ce484222325, // FNV offset basis
        }
    }

    #[inline]
    fn hash<T: Hash + ?Sized>(value: &T) -> u64 {
        let mut hasher = Self::new();
        value.hash(&mut hasher);
        hasher.finish()
    }
}

impl Hasher for FnvHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.state
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        const FNV_PRIME: u64 = 0x100000001b3;
        for &byte in bytes {
            self.state ^= byte as u64;
            self.state = self.state.wrapping_mul(FNV_PRIME);
        }
    }
}

/// Bucket dans la table de hachage
#[derive(Clone)]
enum Bucket<K, V> {
    Empty,
    Occupied { key: K, value: V, distance: u32 },
    Tombstone,
}

impl<K, V> Bucket<K, V> {
    #[inline]
    fn is_empty(&self) -> bool {
        matches!(self, Bucket::Empty | Bucket::Tombstone)
    }

    #[inline]
    fn is_occupied(&self) -> bool {
        matches!(self, Bucket::Occupied { .. })
    }
}

/// HashMap haute-performance avec Robin Hood hashing
///
/// # Exemple
/// ```no_run
/// use exo_std::collections::HashMap;
///
/// let mut map = HashMap::new();
/// map.insert("key", 42);
/// assert_eq!(map.get(&"key"), Some(&42));
/// ```
pub struct HashMap<K, V> {
    buckets: Box<[Bucket<K, V>]>,
    len: usize,
    capacity: usize,
}

impl<K, V> HashMap<K, V> {
    /// Crée une nouvelle HashMap vide
    pub fn new() -> Self {
        Self::with_capacity(INITIAL_CAPACITY)
    }

    /// Crée une HashMap avec capacité initiale
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.next_power_of_two().max(INITIAL_CAPACITY);
        let mut buckets = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            buckets.push(Bucket::Empty);
        }

        Self {
            buckets: buckets.into_boxed_slice(),
            len: 0,
            capacity,
        }
    }

    /// Nombre d'éléments
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Vérifie si vide
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Capacity
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Efface tous les éléments
    pub fn clear(&mut self) {
        for bucket in self.buckets.iter_mut() {
            *bucket = Bucket::Empty;
        }
        self.len = 0;
    }
}

impl<K: Hash + Eq, V> HashMap<K, V> {
    /// Insère une paire clé-valeur
    ///
    /// Retourne l'ancienne valeur si la clé existait.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.maybe_resize();

        let hash = FnvHasher::hash(&key);
        let mut index = (hash as usize) & (self.capacity - 1);
        let mut distance = 0u32;

        let mut insert_key = key;
        let mut insert_value = value;
        let mut insert_distance = distance;

        loop {
            match &mut self.buckets[index] {
                Bucket::Empty | Bucket::Tombstone => {
                    // Slot libre, insertion
                    self.buckets[index] = Bucket::Occupied {
                        key: insert_key,
                        value: insert_value,
                        distance: insert_distance,
                    };
                    self.len += 1;
                    return None;
                }
                Bucket::Occupied {
                    key: existing_key,
                    value: existing_value,
                    distance: existing_distance,
                } => {
                    // Si même clé, remplace la valeur
                    if *existing_key == insert_key {
                        return Some(mem::replace(existing_value, insert_value));
                    }

                    // Robin Hood: si notre distance > distance existante, swap
                    if insert_distance > *existing_distance {
                        mem::swap(&mut insert_key, existing_key);
                        mem::swap(&mut insert_value, existing_value);
                        mem::swap(&mut insert_distance, existing_distance);
                    }

                    // Continue probing
                    index = (index + 1) & (self.capacity - 1);
                    insert_distance += 1;
                }
            }
        }
    }

    /// Recherche une valeur par clé
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = FnvHasher::hash(key);
        let mut index = (hash as usize) & (self.capacity - 1);
        let mut distance = 0u32;

        loop {
            match &self.buckets[index] {
                Bucket::Empty => return None,
                Bucket::Tombstone => {
                    // Continue searching
                }
                Bucket::Occupied {
                    key: existing_key,
                    value,
                    distance: existing_distance,
                } => {
                    if existing_key.borrow() == key {
                        return Some(value);
                    }

                    // Si distance actuelle > distance stockée, la clé n'existe pas
                    if distance > *existing_distance {
                        return None;
                    }
                }
            }

            index = (index + 1) & (self.capacity - 1);
            distance += 1;

            // Protection contre boucle infinie
            if distance > self.capacity as u32 {
                return None;
            }
        }
    }

    /// Recherche mutable
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = FnvHasher::hash(key);
        let mut index = (hash as usize) & (self.capacity - 1);
        let mut distance = 0u32;

        // First, find the index where the key is located
        let found_index = loop {
            match &self.buckets[index] {
                Bucket::Empty => return None,
                Bucket::Tombstone => {}
                Bucket::Occupied {
                    key: existing_key,
                    distance: existing_distance,
                    ..
                } => {
                    if <K as Borrow<Q>>::borrow(existing_key) == key {
                        break Some(index);
                    }

                    if distance > *existing_distance {
                        return None;
                    }
                }
            }

            index = (index + 1) & (self.capacity - 1);
            distance += 1;

            if distance > self.capacity as u32 {
                return None;
            }
        };

        // Now access the bucket mutably
        if let Some(idx) = found_index {
            if let Bucket::Occupied { value, .. } = &mut self.buckets[idx] {
                return Some(value);
            }
        }

        None
    }

    /// Vérifie si clé existe
    #[inline]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Retire une clé-valeur
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = FnvHasher::hash(key);
        let mut index = (hash as usize) & (self.capacity - 1);
        let mut distance = 0u32;

        loop {
            // Check what's at this index first without holding a borrow
            let found = match &self.buckets[index] {
                Bucket::Empty => return None,
                Bucket::Tombstone => false,
                Bucket::Occupied {
                    key: existing_key,
                    distance: existing_distance,
                    ..
                } => {
                    if <K as Borrow<Q>>::borrow(existing_key) == key {
                        true
                    } else {
                        if distance > *existing_distance {
                            return None;
                        }
                        false
                    }
                }
            };

            if found {
                // Now do the mutable borrow separately
                let old_bucket = mem::replace(&mut self.buckets[index], Bucket::Tombstone);

                if let Bucket::Occupied { value, .. } = old_bucket {
                    self.len -= 1;
                    return Some(value);
                }
            }

            index = (index + 1) & (self.capacity - 1);
            distance += 1;

            if distance > self.capacity as u32 {
                return None;
            }
        }
    }

    /// Resize si facteur de charge trop élevé
    fn maybe_resize(&mut self) {
        let load = self.len as f32 / self.capacity as f32;

        if load >= LOAD_FACTOR {
            self.resize(self.capacity * 2);
        }
    }

    /// Redimensionne la table
    fn resize(&mut self, new_capacity: usize) {
        let new_capacity = new_capacity.next_power_of_two();

        let mut new_map = Self::with_capacity(new_capacity);

        for bucket in self.buckets.iter_mut() {
            if let Bucket::Occupied { key, value, .. } = mem::replace(bucket, Bucket::Empty) {
                new_map.insert(key, value);
            }
        }

        *self = new_map;
    }

    /// Itère sur les pairs (clé, valeur)
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            buckets: &self.buckets,
            index: 0,
        }
    }

    /// Itère sur les clés
    pub fn keys(&self) -> Keys<'_, K, V> {
        Keys { inner: self.iter() }
    }

    /// Itère sur les valeurs
    pub fn values(&self) -> Values<'_, K, V> {
        Values { inner: self.iter() }
    }
}

impl<K, V> Default for HashMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

/// Itérateur sur pairs (K, V)
pub struct Iter<'a, K, V> {
    buckets: &'a [Bucket<K, V>],
    index: usize,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.buckets.len() {
            let bucket = &self.buckets[self.index];
            self.index += 1;

            if let Bucket::Occupied { key, value, .. } = bucket {
                return Some((key, value));
            }
        }
        None
    }
}

/// Itérateur sur clés
pub struct Keys<'a, K, V> {
    inner: Iter<'a, K, V>,
}

impl<'a, K, V> Iterator for Keys<'a, K, V> {
    type Item = &'a K;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, _)| k)
    }
}

/// Itérateur sur valeurs
pub struct Values<'a, K, V> {
    inner: Iter<'a, K, V>,
}

impl<'a, K, V> Iterator for Values<'a, K, V> {
    type Item = &'a V;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(_, v)| v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hashmap_insert_get() {
        let mut map = HashMap::new();

        assert_eq!(map.insert("key1", 10), None);
        assert_eq!(map.insert("key2", 20), None);
        assert_eq!(map.insert("key1", 15), Some(10)); // Replace

        assert_eq!(map.get(&"key1"), Some(&15));
        assert_eq!(map.get(&"key2"), Some(&20));
        assert_eq!(map.get(&"key3"), None);

        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_hashmap_remove() {
        let mut map = HashMap::new();

        map.insert("a", 1);
        map.insert("b", 2);
        map.insert("c", 3);

        assert_eq!(map.remove(&"b"), Some(2));
        assert_eq!(map.remove(&"b"), None);
        assert_eq!(map.len(), 2);

        assert_eq!(map.get(&"a"), Some(&1));
        assert_eq!(map.get(&"c"), Some(&3));
    }

    #[test]
    fn test_hashmap_resize() {
        let mut map = HashMap::with_capacity(4);

        for i in 0..100 {
            map.insert(i, i * 2);
        }

        for i in 0..100 {
            assert_eq!(map.get(&i), Some(&(i * 2)));
        }

        assert!(map.capacity() >= 100);
    }

    #[test]
    fn test_hashmap_iter() {
        let mut map = HashMap::new();
        map.insert(1, "one");
        map.insert(2, "two");
        map.insert(3, "three");

        let mut count = 0;
        for (_k, _v) in map.iter() {
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[test]
    fn test_hashmap_clear() {
        let mut map = HashMap::new();
        map.insert(1, 10);
        map.insert(2, 20);

        map.clear();
        assert_eq!(map.len(), 0);
        assert!(map.is_empty());
        assert_eq!(map.get(&1), None);
    }
}
