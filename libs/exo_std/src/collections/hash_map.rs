//! HashMap simplifié pour exo_std (no_std compatible)

extern crate alloc;
use alloc::vec::Vec;
use core::borrow::Borrow;
use core::hash::{Hash, Hasher};

/// Capacité initiale
const INITIAL_CAPACITY: usize = 16;

/// Simple hasher (FNV-1a)
struct SimpleHasher {
    state: u64,
}

impl SimpleHasher {
    const fn new() -> Self {
        Self {
            state: 0xcbf29ce484222325,
        }
    }
}

impl Hasher for SimpleHasher {
    fn finish(&self) -> u64 {
        self.state
    }

    fn write(&mut self, bytes: &[u8]) {
        const FNV_PRIME: u64 = 0x100000001b3;
        for &byte in bytes {
            self.state ^= byte as u64;
            self.state = self.state.wrapping_mul(FNV_PRIME);
        }
    }
}

/// HashMap non cryptographique (implémentation simplifiée)
pub struct HashMap<K, V> {
    entries: Vec<(K, V)>,
}

impl<K, V> HashMap<K, V> {
    /// Crée une nouvelle HashMap vide
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Retourne le nombre d'éléments
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Vérifie si la map est vide
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Supprime tous les éléments
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl<K: Eq, V> HashMap<K, V> {
    /// Insère une paire clé-valeur
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        for entry in &mut self.entries {
            if entry.0 == key {
                return Some(core::mem::replace(&mut entry.1, value));
            }
        }
        self.entries.push((key, value));
        None
    }

    /// Récupère une valeur par clé
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.entries
            .iter()
            .find(|(k, _)| k.borrow() == key)
            .map(|(_, v)| v)
    }

    /// Récupère une valeur mutable par clé
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.entries
            .iter_mut()
            .find(|(k, _)| k.borrow() == key)
            .map(|(_, v)| v)
    }

    /// Supprime une valeur par clé
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + ?Sized,
    {
        let position = self.entries.iter().position(|(k, _)| k.borrow() == key)?;
        Some(self.entries.remove(position).1)
    }

    /// Vérifie si contient une clé
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Retourne un itérateur
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            inner: self.entries.iter(),
        }
    }

    /// Retourne un itérateur sur les clés
    pub fn keys(&self) -> Keys<'_, K, V> {
        Keys { inner: self.iter() }
    }

    /// Retourne un itérateur sur les valeurs
    pub fn values(&self) -> Values<'_, K, V> {
        Values { inner: self.iter() }
    }
}

impl<K, V> Default for HashMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

/// Itérateur sur les paires clé-valeur
pub struct Iter<'a, K, V> {
    inner: core::slice::Iter<'a, (K, V)>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, v)| (k, v))
    }
}

/// Itérateur sur les clés
pub struct Keys<'a, K, V> {
    inner: Iter<'a, K, V>,
}

impl<'a, K, V> Iterator for Keys<'a, K, V> {
    type Item = &'a K;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, _)| k)
    }
}

/// Itérateur sur les valeurs
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
    fn test_hashmap_basic() {
        let mut map = HashMap::new();
        assert!(map.is_empty());

        assert_eq!(map.insert("a", 1), None);
        assert_eq!(map.insert("b", 2), None);
        assert_eq!(map.len(), 2);

        assert_eq!(map.get("a"), Some(&1));
        assert_eq!(map.get("b"), Some(&2));
        assert_eq!(map.get("c"), None);
    }

    #[test]
    fn test_hashmap_replace() {
        let mut map = HashMap::new();
        
        map.insert("key", 1);
        assert_eq!(map.insert("key", 2), Some(1));
        assert_eq!(map.get("key"), Some(&2));
    }

    #[test]
    fn test_hashmap_remove() {
        let mut map = HashMap::new();
        
        map.insert("a", 1);
        map.insert("b", 2);

        assert_eq!(map.remove("a"), Some(1));
        assert_eq!(map.remove("a"), None);
        assert_eq!(map.len(), 1);
    }
}
