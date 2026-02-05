<<<<<<< Updated upstream
// libs/exo_std/src/collections/radix_tree.rs
//! Radix tree (compact prefix trie) for memory-efficient key-value storage

use alloc::boxed::Box;
use alloc::vec::Vec;

/// Radix tree node
struct RadixNode<V> {
    /// Prefix stored at this node
    prefix: Vec<u8>,
    /// Value (Some if this is a terminal node)
    value: Option<V>,
    /// Children nodes
    children: Vec<(u8, Box<RadixNode<V>>)>,
}

impl<V> RadixNode<V> {
    fn new(prefix: Vec<u8>) -> Self {
=======
//! Radix tree pour recherche rapide avec préfixes
//!
//! Optimisé pour les clés de type string/bytes avec préfixes communs.

use core::mem;

extern crate alloc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::string::String;

/// Nœud du radix tree
struct RadixNode<V> {
    prefix: String,
    value: Option<V>,
    children: Vec<Box<RadixNode<V>>>,
}

impl<V> RadixNode<V> {
    fn new(prefix: String) -> Self {
>>>>>>> Stashed changes
        Self {
            prefix,
            value: None,
            children: Vec::new(),
        }
    }
<<<<<<< Updated upstream
    
    fn find_child(&self, byte: u8) -> Option<usize> {
        self.children.iter().position(|(b, _)| *b == byte)
    }
}

/// Radix tree for efficient prefix-based lookups
=======

    fn with_value(prefix: String, value: V) -> Self {
        Self {
            prefix,
            value: Some(value),
            children: Vec::new(),
        }
    }
}

/// Radix tree (arbre de préfixes compressé)
>>>>>>> Stashed changes
pub struct RadixTree<V> {
    root: Option<Box<RadixNode<V>>>,
    len: usize,
}

impl<V> RadixTree<V> {
<<<<<<< Updated upstream
    /// Create new radix tree
    pub fn new() -> Self {
=======
    /// Crée un nouveau radix tree vide
    pub const fn new() -> Self {
>>>>>>> Stashed changes
        Self {
            root: None,
            len: 0,
        }
    }
<<<<<<< Updated upstream
    
    /// Insert key-value pair
    pub fn insert(&mut self, key: &[u8], value: V) -> Option<V> {
        if key.is_empty() {
            return None;
        }
        
        if self.root.is_none() {
            let mut node = RadixNode::new(key.to_vec());
            node.value = Some(value);
            self.root = Some(Box::new(node));
            self.len += 1;
            return None;
        }
        
        let len = &mut self.len;
        Self::insert_at_static(self.root.as_mut().unwrap(), key, value, len)
    }
    
    fn insert_at_static(node: &mut RadixNode<V>, key: &[u8], value: V, len: &mut usize) -> Option<V> {
        // Find common prefix length
        let common_len = node.prefix.iter()
            .zip(key.iter())
            .take_while(|(a, b)| a == b)
            .count();
        
        if common_len == node.prefix.len() {
            // Key extends current prefix
            if common_len == key.len() {
                // Exact match - replace value
                let old = node.value.take();
                node.value = Some(value);
                if old.is_none() {
                    *len += 1;
                }
                return old;
            }
            
            // Continue to child
            let rest = &key[common_len..];
            let first_byte = rest[0];
            
            if let Some(idx) = node.find_child(first_byte) {
                return Self::insert_at_static(&mut node.children[idx].1, rest, value, len);
            }
            
            // Create new child
            let mut child = RadixNode::new(rest.to_vec());
            child.value = Some(value);
            node.children.push((first_byte, Box::new(child)));
            *len += 1;
            None
        } else {
            // Need to split node
            let old_prefix = node.prefix.split_off(common_len);
            let old_value = node.value.take();
            let old_children = core::mem::take(&mut node.children);
            
            // Create old branch
            if !old_prefix.is_empty() {
                let first_byte = old_prefix[0];
                let mut old_branch = RadixNode::new(old_prefix);
                old_branch.value = old_value;
                old_branch.children = old_children;
                node.children.push((first_byte, Box::new(old_branch)));
            }
            
            // Create new branch
            if common_len < key.len() {
                let new_prefix = key[common_len..].to_vec();
                let first_byte = new_prefix[0];
                let mut new_branch = RadixNode::new(new_prefix);
                new_branch.value = Some(value);
                node.children.push((first_byte, Box::new(new_branch)));
                *len += 1;
            } else {
                node.value = Some(value);
                *len += 1;
            }
            
            None
        }
    }
    
    /// Get value by key
    pub fn get(&self, key: &[u8]) -> Option<&V> {
        let root = self.root.as_ref()?;
        self.get_at(root, key)
    }
    
    fn get_at<'a>(&self, node: &'a RadixNode<V>, key: &[u8]) -> Option<&'a V> {
        if !key.starts_with(&node.prefix) {
            return None;
        }
        
        let remaining = &key[node.prefix.len()..];
        
        if remaining.is_empty() {
            return node.value.as_ref();
        }
        
        let first_byte = remaining[0];
        let idx = node.find_child(first_byte)?;
        self.get_at(&node.children[idx].1, remaining)
    }
    
    /// Check if key exists
    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }
    
    /// Get length
    pub const fn len(&self) -> usize {
        self.len
    }
    
    /// Check if empty
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    
    /// Clear tree
=======

    /// Retourne le nombre d'éléments
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Vérifie si vide
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Insère une clé-valeur
    pub fn insert(&mut self, key: &str, value: V) -> Option<V> {
        if key.is_empty() {
            return None;
        }

        if self.root.is_none() {
            self.root = Some(Box::new(RadixNode::with_value(
                String::from(key),
                value,
            )));
            self.len = 1;
            return None;
        }

        let old = self.insert_recursive(self.root.as_mut().unwrap(), key, value);
        if old.is_none() {
            self.len += 1;
        }
        old
    }

    fn insert_recursive(
        &mut self,
        node: &mut Box<RadixNode<V>>,
        key: &str,
        value: V,
    ) -> Option<V> {
        let common = common_prefix(&node.prefix, key);

        if common == 0 {
            // Aucun préfixe commun, cherche dans enfants
            let remaining = key;
            for child in &mut node.children {
                let child_common = common_prefix(&child.prefix, remaining);
                if child_common > 0 {
                    return self.insert_recursive(child, remaining, value);
                }
            }
            // Ajouter nouveau comme enfant
            node.children.push(Box::new(RadixNode::with_value(
                String::from(key),
                value,
            )));
            None
        } else if common == node.prefix.len() && common == key.len() {
            // Exact match
            node.value.replace(value)
        } else if common == node.prefix.len() {
            // Le préfixe du nœud est entièrement dans la clé
            let remaining = &key[common..];
            for child in &mut node.children {
                let child_common = common_prefix(&child.prefix, remaining);
                if child_common > 0 {
                    return self.insert_recursive(child, remaining, value);
                }
            }
            // Ajouter comme nouvel enfant
            node.children.push(Box::new(RadixNode::with_value(
                String::from(remaining),
                value,
            )));
            None
        } else {
            // Split du nœud
            let old_prefix = mem::replace(&mut node.prefix, String::from(&key[..common]));
            let old_value = node.value.take();
            let old_children = mem::take(&mut node.children);

            // Ancien enfant
            let mut old_child = Box::new(RadixNode {
                prefix: String::from(&old_prefix[common..]),
                value: old_value,
                children: old_children,
            });

            if common == key.len() {
                // La nouvelle clé devient le nœud parent
                node.value = Some(value);
                node.children.push(old_child);
                None
            } else {
                // Deux enfants : ancien + nouveau
                let new_child = Box::new(RadixNode::with_value(
                    String::from(&key[common..]),
                    value,
                ));
                node.children.push(old_child);
                node.children.push(new_child);
                None
            }
        }
    }

    /// Récupère une valeur
    pub fn get(&self, key: &str) -> Option<&V> {
        if key.is_empty() || self.root.is_none() {
            return None;
        }

        self.get_recursive(self.root.as_ref().unwrap(), key)
    }

    fn get_recursive<'a>(&'a self, node: &'a RadixNode<V>, key: &str) -> Option<&'a V> {
        let common = common_prefix(&node.prefix, key);

        if common == 0 {
            return None;
        }

        if common == node.prefix.len() && common == key.len() {
            return node.value.as_ref();
        }

        if common == node.prefix.len() {
            let remaining = &key[common..];
            for child in &node.children {
                if let Some(value) = self.get_recursive(child, remaining) {
                    return Some(value);
                }
            }
        }

        None
    }

    /// Supprime tous les éléments
>>>>>>> Stashed changes
    pub fn clear(&mut self) {
        self.root = None;
        self.len = 0;
    }
}

impl<V> Default for RadixTree<V> {
    fn default() -> Self {
        Self::new()
    }
}

<<<<<<< Updated upstream
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_radix_tree_basic() {
        let mut tree = RadixTree::new();
        
        tree.insert(b"test", 1);
        tree.insert(b"testing", 2);
        tree.insert(b"toast", 3);
        
        assert_eq!(tree.get(b"test"), Some(&1));
        assert_eq!(tree.get(b"testing"), Some(&2));
        assert_eq!(tree.get(b"toast"), Some(&3));
        assert_eq!(tree.get(b"unknown"), None);
        
        assert_eq!(tree.len(), 3);
    }
    
    #[test]
    fn test_radix_tree_replace() {
        let mut tree = RadixTree::new();
        
        assert_eq!(tree.insert(b"key", 1), None);
        assert_eq!(tree.insert(b"key", 2), Some(1));
        assert_eq!(tree.get(b"key"), Some(&2));
        assert_eq!(tree.len(), 1);
=======
/// Calcule la longueur du préfixe commun
fn common_prefix(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(ca, cb)| ca == cb)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_radix_tree() {
        let mut tree = RadixTree::new();

        tree.insert("test", 1);
        tree.insert("testing", 2);
        tree.insert("toast", 3);

        assert_eq!(tree.get("test"), Some(&1));
        assert_eq!(tree.get("testing"), Some(&2));
        assert_eq!(tree.get("toast"), Some(&3));
        assert_eq!(tree.get("tea"), None);
    }

    #[test]
    fn test_common_prefix() {
        assert_eq!(common_prefix("hello", "help"), 3);
        assert_eq!(common_prefix("abc", "xyz"), 0);
        assert_eq!(common_prefix("test", "test"), 4);
>>>>>>> Stashed changes
    }
}
