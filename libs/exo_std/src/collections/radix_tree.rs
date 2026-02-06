// libs/exo_std/src/collections/radix_tree.rs
//! Radix tree (compact prefix trie) for memory-efficient key-value storage
//!
//! Optimized for fast lookups with common prefixes.

use alloc::boxed::Box;
use alloc::vec::Vec;

/// Radix tree node
struct RadixNode<V> {
    /// Prefix stored at this node
    prefix: Vec<u8>,
    /// Value (Some if this is a terminal node)
    value: Option<V>,
    /// Children nodes (first byte, node)
    children: Vec<(u8, Box<RadixNode<V>>)>,
}

impl<V> RadixNode<V> {
    fn new(prefix: Vec<u8>) -> Self {
        Self {
            prefix,
            value: None,
            children: Vec::new(),
        }
    }

    fn with_value(prefix: Vec<u8>, value: V) -> Self {
        Self {
            prefix,
            value: Some(value),
            children: Vec::new(),
        }
    }

    fn find_child(&self, byte: u8) -> Option<usize> {
        self.children.iter().position(|(b, _)| *b == byte)
    }
}

/// Radix tree for efficient prefix-based lookups
pub struct RadixTree<V> {
    root: Option<Box<RadixNode<V>>>,
    len: usize,
}

impl<V> RadixTree<V> {
    /// Create new radix tree
    pub const fn new() -> Self {
        Self {
            root: None,
            len: 0,
        }
    }

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

    /// Get mutable value by key
    pub fn get_mut(&mut self, key: &[u8]) -> Option<&mut V> {
        let root = self.root.as_mut()?;
        Self::get_mut_at(root, key)
    }

    fn get_mut_at<'a>(node: &'a mut RadixNode<V>, key: &[u8]) -> Option<&'a mut V> {
        if !key.starts_with(&node.prefix) {
            return None;
        }

        let remaining = &key[node.prefix.len()..];

        if remaining.is_empty() {
            return node.value.as_mut();
        }

        let first_byte = remaining[0];
        let idx = node.find_child(first_byte)?;
        Self::get_mut_at(&mut node.children[idx].1, remaining)
    }

    /// Check if key exists
    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }

    /// Remove key-value pair
    pub fn remove(&mut self, key: &[u8]) -> Option<V> {
        if key.is_empty() || self.root.is_none() {
            return None;
        }

        let result = Self::remove_at(self.root.as_mut().unwrap(), key);
        if result.is_some() {
            self.len -= 1;
        }
        result
    }

    fn remove_at(node: &mut RadixNode<V>, key: &[u8]) -> Option<V> {
        if !key.starts_with(&node.prefix) {
            return None;
        }

        let remaining = &key[node.prefix.len()..];

        if remaining.is_empty() {
            return node.value.take();
        }

        let first_byte = remaining[0];
        let idx = node.find_child(first_byte)?;
        Self::remove_at(&mut node.children[idx].1, remaining)
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
    }

    #[test]
    fn test_radix_tree_remove() {
        let mut tree = RadixTree::new();

        tree.insert(b"test", 1);
        tree.insert(b"testing", 2);

        assert_eq!(tree.remove(b"test"), Some(1));
        assert_eq!(tree.get(b"test"), None);
        assert_eq!(tree.get(b"testing"), Some(&2));
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn test_radix_tree_contains() {
        let mut tree = RadixTree::new();

        tree.insert(b"hello", 42);
        assert!(tree.contains_key(b"hello"));
        assert!(!tree.contains_key(b"world"));
    }
}
