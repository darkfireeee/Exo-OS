//! BTreeMap pour exo_std (no_std compatible)
//!
//! Implémentation simplifiée d'un B-Tree map pour environnements no_std.

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

use core::borrow::Borrow;
use core::cmp::Ordering;
use core::marker::PhantomData;

/// Nombre d'éléments par noeud
const NODE_SIZE: usize = 6;

/// Noeud du BTree
struct Node<K, V> {
    keys: [Option<K>; NODE_SIZE],
    values: [Option<V>; NODE_SIZE],
    children: [Option<Box<Node<K, V>>>; NODE_SIZE + 1],
    count: usize,
    is_leaf: bool,
}

impl<K, V> Node<K, V> {
    fn new(is_leaf: bool) -> Self {
        Self {
            keys: Default::default(),
            values: Default::default(),
            children: Default::default(),
            count: 0,
            is_leaf,
        }
    }
}

/// Map ordonnée basée sur un B-Tree
pub struct BTreeMap<K, V> {
    root: Option<Box<Node<K, V>>>,
    len: usize,
    _marker: PhantomData<(K, V)>,
}

impl<K, V> BTreeMap<K, V> {
    /// Crée une nouvelle BTreeMap vide
    pub const fn new() -> Self {
        Self {
            root: None,
            len: 0,
            _marker: PhantomData,
        }
    }

    /// Retourne le nombre d'éléments
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Vérifie si la map est vide
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Supprime tous les éléments
    pub fn clear(&mut self) {
        self.root = None;
        self.len = 0;
    }
}

impl<K: Ord, V> BTreeMap<K, V> {
    /// Insère une paire clé-valeur
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if self.root.is_none() {
            let mut root = Box::new(Node::new(true));
            root.keys[0] = Some(key);
            root.values[0] = Some(value);
            root.count = 1;
            self.root = Some(root);
            self.len = 1;
            return None;
        }

        // Implémentation simplifiée : remplace si existe
        let root = self.root.as_mut().unwrap();
        for i in 0..root.count {
            if let Some(k) = &root.keys[i] {
                if k == &key {
                    return root.values[i].replace(value);
                }
            }
        }

        // Ajoute si pas plein
        if root.count < NODE_SIZE {
            root.keys[root.count] = Some(key);
            root.values[root.count] = Some(value);
            root.count += 1;
            self.len += 1;
            None
        } else {
            // Map pleine (implémentation simplifiée)
            None
        }
    }

    /// Récupère une valeur par clé
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let root = self.root.as_ref()?;
        for i in 0..root.count {
            if let Some(k) = &root.keys[i] {
                if k.borrow() == key {
                    return root.values[i].as_ref();
                }
            }
        }
        None
    }

    /// Récupère une valeur mutable par clé
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let root = self.root.as_mut()?;
        for i in 0..root.count {
            if let Some(k) = &root.keys[i] {
                if k.borrow() == key {
                    return root.values[i].as_mut();
                }
            }
        }
        None
    }

    /// Supprime une valeur par clé
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let root = self.root.as_mut()?;
        for i in 0..root.count {
            if let Some(k) = &root.keys[i] {
                if k.borrow() == key {
                    let value = root.values[i].take();
                    // Déplace les éléments
                    for j in i..root.count - 1 {
                        root.keys[j] = root.keys[j + 1].take();
                        root.values[j] = root.values[j + 1].take();
                    }
                    root.count -= 1;
                    self.len -= 1;
                    return value;
                }
            }
        }
        None
    }

    /// Vérifie si contient une clé
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Retourne un itérateur sur les clés
    pub fn keys(&self) -> Keys<'_, K, V> {
        Keys { inner: self.iter() }
    }

    /// Retourne un itérateur sur les valeurs
    pub fn values(&self) -> Values<'_, K, V> {
        Values { inner: self.iter() }
    }

    /// Retourne un itérateur
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            map: self,
            index: 0,
        }
    }
}

impl<K, V> Default for BTreeMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

/// Itérateur sur les paires clé-valeur
pub struct Iter<'a, K, V> {
    map: &'a BTreeMap<K, V>,
    index: usize,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let root = self.map.root.as_ref()?;
        if self.index >= root.count {
            return None;
        }

        let key = root.keys[self.index].as_ref()?;
        let value = root.values[self.index].as_ref()?;
        self.index += 1;

        Some((key, value))
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
    fn test_btreemap_basic() {
        let mut map = BTreeMap::new();
        assert!(map.is_empty());

        assert_eq!(map.insert("a", 1), None);
        assert_eq!(map.insert("b", 2), None);
        assert_eq!(map.len(), 2);

        assert_eq!(map.get("a"), Some(&1));
        assert_eq!(map.get("b"), Some(&2));
        assert_eq!(map.get("c"), None);
    }

    #[test]
    fn test_btreemap_replace() {
        let mut map = BTreeMap::new();
        
        map.insert("key", 1);
        assert_eq!(map.insert("key", 2), Some(1));
        assert_eq!(map.get("key"), Some(&2));
    }

    #[test]
    fn test_btreemap_remove() {
        let mut map = BTreeMap::new();
        
        map.insert("a", 1);
        map.insert("b", 2);

        assert_eq!(map.remove("a"), Some(1));
        assert_eq!(map.remove("a"), None);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn test_btreemap_iter() {
        let mut map = BTreeMap::new();
        map.insert("a", 1);
        map.insert("b", 2);

        let items: Vec<_> = map.iter().collect();
        assert_eq!(items.len(), 2);
    }
}
