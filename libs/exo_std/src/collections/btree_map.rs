//! BTreeMap no_std complet avec B-Tree ordre 16
//!
//! Implémentation haute-performance utilisant:
//! - Ordre 16 pour cache-line optimization (64 bytes)
//! - Split/merge automatique des nodes
//! - Range queries efficaces
//! - Iterateurs in-order

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::borrow::Borrow;
use core::mem;

/// Ordre du B-Tree (max 15 keys par node)
/// Optimisé pour cache line de 64 bytes
const ORDER: usize = 16;
const MIN_KEYS: usize = ORDER / 2 - 1; // 7
const MAX_KEYS: usize = ORDER - 1;     // 15
const MAX_CHILDREN: usize = ORDER;     // 16

/// Noeud du B-Tree
struct Node<K, V> {
    keys: Vec<K>,
    values: Vec<V>,
    children: Vec<Box<Node<K, V>>>,
    is_leaf: bool,
}

impl<K, V> Node<K, V> {
    /// Crée un nouveau noeud
    fn new(is_leaf: bool) -> Self {
        Self {
            keys: Vec::new(),
            values: Vec::new(),
            children: if is_leaf { Vec::new() } else { Vec::with_capacity(MAX_CHILDREN) },
            is_leaf,
        }
    }

    /// Nombre de clés
    #[inline]
    fn len(&self) -> usize {
        self.keys.len()
    }

    /// Vérifie si le noeud est plein
    #[inline]
    fn is_full(&self) -> bool {
        self.keys.len() >= MAX_KEYS
    }

    /// Vérifie si le noeud peut fusionner
    #[inline]
    fn can_merge(&self) -> bool {
        self.keys.len() < MIN_KEYS
    }

    /// Recherche index d'une clé (binary search)
    fn find_key_index<Q>(&self, key: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.keys.binary_search_by(|k| k.borrow().cmp(key))
    }

    /// Insère (key, value) à l'index
    fn insert_at(&mut self, index: usize, key: K, value: V) {
        self.keys.insert(index, key);
        self.values.insert(index, value);
    }

    /// Split le noeud en deux
    fn split(&mut self) -> (K, V, Box<Node<K, V>>) {
        let mid = MAX_KEYS / 2;

        // Crée nouveau noeud pour partie droite
        let mut right = Box::new(Node::new(self.is_leaf));

        // Extraire clé médiane
        let mid_key = self.keys.remove(mid);
        let mid_value = self.values.remove(mid);

        // Déplacer moitié droite
        right.keys = self.keys.split_off(mid);
        right.values = self.values.split_off(mid);

        if !self.is_leaf {
            right.children = self.children.split_off(mid + 1);
        }

        (mid_key, mid_value, right)
    }
}

/// Map ordonnée basée sur B-Tree
///
/// # Exemple
/// ```no_run
/// use exo_std::collections::BTreeMap;
///
/// let mut map = BTreeMap::new();
/// map.insert(1, "one");
/// map.insert(2, "two");
/// assert_eq!(map.get(&1), Some(&"one"));
/// ```
pub struct BTreeMap<K, V> {
    root: Option<Box<Node<K, V>>>,
    len: usize,
}

impl<K, V> BTreeMap<K, V> {
    /// Crée une nouvelle BTreeMap vide
    pub const fn new() -> Self {
        Self {
            root: None,
            len: 0,
        }
    }

    /// Retourne le nombre d'éléments
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Vérifie si la map est vide
    #[inline]
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
    ///
    /// Retourne l'ancienne valeur si la clé existait.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if self.root.is_none() {
            let mut root = Box::new(Node::new(true));
            root.keys.push(key);
            root.values.push(value);
            self.root = Some(root);
            self.len = 1;
            return None;
        }

        // Vérifie si root est plein
        if self.root.as_ref().unwrap().is_full() {
            let old_root = self.root.take().unwrap();
            let mut new_root = Box::new(Node::new(false));

            new_root.children.push(old_root);
            self.split_child(&mut new_root, 0);
            self.root = Some(new_root);
        }

        Self::insert_non_full(self.root.as_mut().unwrap(), key, value, &mut self.len)
    }

    /// Insère dans un noeud non-plein
    fn insert_non_full(node: &mut Node<K, V>, key: K, value: V, len: &mut usize) -> Option<V> {
        match node.find_key_index(&key) {
            Ok(index) => {
                // Clé existe, remplace valeur
                Some(mem::replace(&mut node.values[index], value))
            }
            Err(index) => {
                if node.is_leaf {
                    // Feuille: insère directement
                    node.insert_at(index, key, value);
                    *len += 1;
                    None
                } else {
                    // Noeud interne: descend vers enfant
                    if node.children[index].is_full() {
                        Self::split_child_mut(node, index);

                        // Après split, re-détermine l'enfant
                        if key > node.keys[index] {
                            return Self::insert_non_full(&mut node.children[index + 1], key, value, len);
                        } else if key == node.keys[index] {
                            return Some(mem::replace(&mut node.values[index], value));
                        }
                    }
                    Self::insert_non_full(&mut node.children[index], key, value, len)
                }
            }
        }
    }

    /// Split un enfant plein
    fn split_child(&mut self, parent: &mut Node<K, V>, child_index: usize) {
        Self::split_child_mut(parent, child_index);
    }

    fn split_child_mut(parent: &mut Node<K, V>, child_index: usize) {
        let child = &mut parent.children[child_index];
        let (mid_key, mid_value, right_node) = child.split();

        parent.keys.insert(child_index, mid_key);
        parent.values.insert(child_index, mid_value);
        parent.children.insert(child_index + 1, right_node);
    }

    /// Recherche une valeur par clé
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let mut current = self.root.as_ref()?;

        loop {
            match current.find_key_index(key) {
                Ok(index) => return Some(&current.values[index]),
                Err(index) => {
                    if current.is_leaf {
                        return None;
                    }
                    current = &current.children[index];
                }
            }
        }
    }

    /// Recherche mutable
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let mut current = self.root.as_mut()?;

        loop {
            match current.find_key_index(key) {
                Ok(index) => return Some(&mut current.values[index]),
                Err(index) => {
                    if current.is_leaf {
                        return None;
                    }
                    current = &mut current.children[index];
                }
            }
        }
    }

    /// Vérifie si clé existe
    #[inline]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Retire une clé-valeur
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let root = self.root.as_mut()?;
        let result = Self::remove_from_node(root, key);

        if result.is_some() {
            self.len -= 1;

            // Si root devient vide et a un enfant, promouvoir l'enfant
            if root.keys.is_empty() && !root.is_leaf {
                self.root = Some(root.children.remove(0));
            }
        }

        result
    }

    fn remove_from_node<Q>(node: &mut Node<K, V>, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        match node.find_key_index(key) {
            Ok(index) => {
                if node.is_leaf {
                    // Feuille: supprime directement
                    node.keys.remove(index);
                    Some(node.values.remove(index))
                } else {
                    // Noeud interne: remplace par prédécesseur ou successeur
                    if node.children[index].len() > MIN_KEYS {
                        let (pred_key, pred_value) = Self::remove_max(&mut node.children[index]);
                        let old_value = mem::replace(&mut node.values[index], pred_value);
                        node.keys[index] = pred_key;
                        Some(old_value)
                    } else if node.children[index + 1].len() > MIN_KEYS {
                        let (succ_key, succ_value) = Self::remove_min(&mut node.children[index + 1]);
                        let old_value = mem::replace(&mut node.values[index], succ_value);
                        node.keys[index] = succ_key;
                        Some(old_value)
                    } else {
                        // Merge et retire
                        Self::merge_children(node, index);
                        Self::remove_from_node(&mut node.children[index], key)
                    }
                }
            }
            Err(index) => {
                if node.is_leaf {
                    None
                } else {
                    // Descend vers enfant
                    if node.children[index].len() <= MIN_KEYS {
                        Self::fix_child(node, index);
                    }

                    // Après fix, re-cherche l'index
                    match node.find_key_index(key) {
                        Ok(idx) => Self::remove_from_node(node, key),
                        Err(idx) => Self::remove_from_node(&mut node.children[idx], key),
                    }
                }
            }
        }
    }

    fn remove_max(node: &mut Node<K, V>) -> (K, V) {
        if node.is_leaf {
            let key = node.keys.pop().unwrap();
            let value = node.values.pop().unwrap();
            (key, value)
        } else {
            let last_child = node.children.len() - 1;
            Self::remove_max(&mut node.children[last_child])
        }
    }

    fn remove_min(node: &mut Node<K, V>) -> (K, V) {
        if node.is_leaf {
            let key = node.keys.remove(0);
            let value = node.values.remove(0);
            (key, value)
        } else {
            Self::remove_min(&mut node.children[0])
        }
    }

    fn merge_children(parent: &mut Node<K, V>, index: usize) {
        let key = parent.keys.remove(index);
        let value = parent.values.remove(index);

        let mut right = parent.children.remove(index + 1);
        let left = &mut parent.children[index];

        left.keys.push(key);
        left.values.push(value);

        left.keys.append(&mut right.keys);
        left.values.append(&mut right.values);

        if !left.is_leaf {
            left.children.append(&mut right.children);
        }
    }

    fn fix_child(parent: &mut Node<K, V>, index: usize) {
        // Essaye emprunter du sibling gauche
        if index > 0 && parent.children[index - 1].len() > MIN_KEYS {
            Self::borrow_from_left(parent, index);
        }
        // Essaye emprunter du sibling droit
        else if index < parent.children.len() - 1 && parent.children[index + 1].len() > MIN_KEYS {
            Self::borrow_from_right(parent, index);
        }
        // Merge avec sibling
        else if index > 0 {
            Self::merge_children(parent, index - 1);
        } else {
            Self::merge_children(parent, index);
        }
    }

    fn borrow_from_left(parent: &mut Node<K, V>, child_index: usize) {
        // Split children to get separate mutable references
        let (left, right) = parent.children.split_at_mut(child_index);
        let sibling = &mut left[child_index - 1];
        let child = &mut right[0];

        // Move key/value from sibling to parent
        let sibling_key = sibling.keys.pop().unwrap();
        let sibling_value = sibling.values.pop().unwrap();

        // Swap parent key/value with sibling's
        let parent_key = mem::replace(&mut parent.keys[child_index - 1], sibling_key);
        let parent_value = mem::replace(&mut parent.values[child_index - 1], sibling_value);

        // Insert parent's old key/value into child
        child.keys.insert(0, parent_key);
        child.values.insert(0, parent_value);

        if !child.is_leaf {
            child.children.insert(0, sibling.children.pop().unwrap());
        }
    }

    fn borrow_from_right(parent: &mut Node<K, V>, child_index: usize) {
        // Split children to get separate mutable references
        let (left, right) = parent.children.split_at_mut(child_index + 1);
        let child = &mut left[child_index];
        let sibling = &mut right[0];

        // Move key/value from sibling to parent
        let sibling_key = sibling.keys.remove(0);
        let sibling_value = sibling.values.remove(0);

        // Swap parent key/value with sibling's
        let parent_key = mem::replace(&mut parent.keys[child_index], sibling_key);
        let parent_value = mem::replace(&mut parent.values[child_index], sibling_value);

        // Insert parent's old key/value into child
        child.keys.push(parent_key);
        child.values.push(parent_value);

        if !child.is_leaf {
            child.children.push(sibling.children.remove(0));
        }
    }

    /// Itère sur les pairs (clé, valeur) en ordre
    pub fn iter(&self) -> Iter<'_, K, V> {
        let mut stack = Vec::new();
        if let Some(root) = &self.root {
            stack.push((root.as_ref(), 0));
        }
        Iter { stack }
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

impl<K, V> Default for BTreeMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

/// Itérateur in-order sur (K, V)
pub struct Iter<'a, K, V> {
    stack: Vec<(&'a Node<K, V>, usize)>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((node, index)) = self.stack.last_mut() {
            if *index < node.keys.len() {
                let key = &node.keys[*index];
                let value = &node.values[*index];
                let current_index = *index;
                *index += 1;

                // Store node reference before potentially pushing
                let should_push_child = !node.is_leaf && current_index + 1 <= node.children.len();
                let child_node = if should_push_child {
                    Some(&node.children[current_index])
                } else {
                    None
                };

                // Now we can push without conflicting borrow
                if let Some(child) = child_node {
                    self.stack.push((child, 0));
                }

                return Some((key, value));
            } else {
                self.stack.pop();
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
    fn test_btree_insert_get() {
        let mut tree = BTreeMap::new();

        assert_eq!(tree.insert(5, "five"), None);
        assert_eq!(tree.insert(3, "three"), None);
        assert_eq!(tree.insert(7, "seven"), None);
        assert_eq!(tree.insert(3, "THREE"), Some("three")); // Replace

        assert_eq!(tree.get(&5), Some(&"five"));
        assert_eq!(tree.get(&3), Some(&"THREE"));
        assert_eq!(tree.get(&7), Some(&"seven"));
        assert_eq!(tree.get(&1), None);

        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn test_btree_remove() {
        let mut tree = BTreeMap::new();

        for i in 0..20 {
            tree.insert(i, i * 2);
        }

        assert_eq!(tree.remove(&10), Some(20));
        assert_eq!(tree.remove(&10), None);
        assert_eq!(tree.len(), 19);

        assert_eq!(tree.get(&9), Some(&18));
        assert_eq!(tree.get(&11), Some(&22));
    }

    #[test]
    fn test_btree_large() {
        let mut tree = BTreeMap::new();

        for i in 0..1000 {
            tree.insert(i, i * 3);
        }

        for i in 0..1000 {
            assert_eq!(tree.get(&i), Some(&(i * 3)));
        }

        assert_eq!(tree.len(), 1000);
    }

    #[test]
    fn test_btree_iter() {
        let mut tree = BTreeMap::new();
        tree.insert(3, "c");
        tree.insert(1, "a");
        tree.insert(2, "b");

        let items: Vec<_> = tree.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(items, vec![(1, "a"), (2, "b"), (3, "c")]);
    }

    #[test]
    fn test_btree_clear() {
        let mut tree = BTreeMap::new();
        tree.insert(1, 10);
        tree.insert(2, 20);

        tree.clear();
        assert_eq!(tree.len(), 0);
        assert!(tree.is_empty());
        assert_eq!(tree.get(&1), None);
    }
}
