// libs/exo_std/src/collections/intrusive_list.rs
//! Intrusive doubly-linked list
//!
//! Nodes are embedded in parent structures for zero-allocation insertions.

use core::ptr::NonNull;
use core::marker::PhantomData;

/// Node for intrusive linked list
#[repr(C)]
pub struct IntrusiveNode<T> {
    next: Option<NonNull<IntrusiveNode<T>>>,
    prev: Option<NonNull<IntrusiveNode<T>>>,
    _marker: PhantomData<T>,
}

impl<T> IntrusiveNode<T> {
    /// Create new unlinked node
    pub const fn new() -> Self {
        Self {
            next: None,
            prev: None,
            _marker: PhantomData,
        }
    }

    /// Initialize the node
    pub fn init(&mut self) {
        self.next = None;
        self.prev = None;
    }

    /// Check if node is linked
    pub fn is_linked(&self) -> bool {
        self.next.is_some() || self.prev.is_some()
    }
}

/// Intrusive doubly-linked list
pub struct IntrusiveList<T> {
    head: Option<NonNull<IntrusiveNode<T>>>,
    tail: Option<NonNull<IntrusiveNode<T>>>,
    len: usize,
}

impl<T> IntrusiveList<T> {
    /// Create new empty list
    pub const fn new() -> Self {
        Self {
            head: None,
            tail: None,
            len: 0,
        }
    }

    /// Push front (O(1))
    ///
    /// # Safety
    /// - `node` must be part of a valid `T` object
    /// - Node must not already be in a list
    /// - Object containing node must outlive the list
    pub unsafe fn push_front(&mut self, node: NonNull<IntrusiveNode<T>>) {
        let node_ref = node.as_ref();
        debug_assert!(!node_ref.is_linked(), "node already linked");

        let node_ptr = node.as_ptr();

        (*node_ptr).next = self.head;
        (*node_ptr).prev = None;

        if let Some(mut head) = self.head {
            head.as_mut().prev = Some(node);
        } else {
            self.tail = Some(node);
        }

        self.head = Some(node);
        self.len += 1;
    }

    /// Push back (O(1))
    ///
    /// # Safety
    /// Same as push_front
    pub unsafe fn push_back(&mut self, node: NonNull<IntrusiveNode<T>>) {
        let node_ref = node.as_ref();
        debug_assert!(!node_ref.is_linked(), "node already linked");

        let node_ptr = node.as_ptr();

        (*node_ptr).prev = self.tail;
        (*node_ptr).next = None;

        if let Some(mut tail) = self.tail {
            tail.as_mut().next = Some(node);
        } else {
            self.head = Some(node);
        }

        self.tail = Some(node);
        self.len += 1;
    }

    /// Pop front (O(1))
    pub fn pop_front(&mut self) -> Option<NonNull<IntrusiveNode<T>>> {
        let node = self.head?;

        unsafe {
            let node_ref = node.as_ref();
            self.head = node_ref.next;

            if let Some(mut next) = node_ref.next {
                next.as_mut().prev = None;
            } else {
                self.tail = None;
            }

            // Clear node links
            let node_ptr = node.as_ptr();
            (*node_ptr).next = None;
            (*node_ptr).prev = None;
        }

        self.len -= 1;
        Some(node)
    }

    /// Pop back (O(1))
    pub fn pop_back(&mut self) -> Option<NonNull<IntrusiveNode<T>>> {
        let node = self.tail?;

        unsafe {
            let node_ref = node.as_ref();
            self.tail = node_ref.prev;

            if let Some(mut prev) = node_ref.prev {
                prev.as_mut().next = None;
            } else {
                self.head = None;
            }

            // Clear node links
            let node_ptr = node.as_ptr();
            (*node_ptr).next = None;
            (*node_ptr).prev = None;
        }

        self.len -= 1;
        Some(node)
    }

    /// Remove specific node (O(1))
    ///
    /// # Safety
    /// Node must be in this list
    pub unsafe fn remove(&mut self, node: NonNull<IntrusiveNode<T>>) {
        let node_ref = node.as_ref();

        match (node_ref.prev, node_ref.next) {
            (Some(mut prev), Some(mut next)) => {
                // Middle node
                prev.as_mut().next = Some(next);
                next.as_mut().prev = Some(prev);
            }
            (Some(mut prev), None) => {
                // Tail node
                prev.as_mut().next = None;
                self.tail = Some(prev);
            }
            (None, Some(mut next)) => {
                // Head node
                next.as_mut().prev = None;
                self.head = Some(next);
            }
            (None, None) => {
                // Only node
                self.head = None;
                self.tail = None;
            }
        }

        // Clear node links
        let node_ptr = node.as_ptr();
        (*node_ptr).next = None;
        (*node_ptr).prev = None;

        self.len -= 1;
    }

    /// Get front node
    pub fn front(&self) -> Option<NonNull<IntrusiveNode<T>>> {
        self.head
    }

    /// Get back node
    pub fn back(&self) -> Option<NonNull<IntrusiveNode<T>>> {
        self.tail
    }

    /// Get length
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Check if empty
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Clear list (unlink all nodes)
    pub fn clear(&mut self) {
        while self.pop_front().is_some() {}
    }

    /// Create an iterator over the list
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            head: self.head,
            tail: self.tail,
            len: self.len,
            _marker: PhantomData,
        }
    }

    /// Create a mutable iterator over the list
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            head: self.head,
            tail: self.tail,
            len: self.len,
            _marker: PhantomData,
        }
    }

    /// Create a cursor positioned at the front
    pub fn cursor_front(&self) -> Cursor<'_, T> {
        Cursor {
            current: self.head,
            list: self,
        }
    }

    /// Create a cursor positioned at the back
    pub fn cursor_back(&self) -> Cursor<'_, T> {
        Cursor {
            current: self.tail,
            list: self,
        }
    }

    /// Create a mutable cursor positioned at the front
    pub fn cursor_front_mut(&mut self) -> CursorMut<'_, T> {
        CursorMut {
            current: self.head,
            list: self,
        }
    }

    /// Create a mutable cursor positioned at the back
    pub fn cursor_back_mut(&mut self) -> CursorMut<'_, T> {
        CursorMut {
            current: self.tail,
            list: self,
        }
    }

    /// Append another list to the end of this list
    ///
    /// After this operation, `other` will be empty.
    pub fn append(&mut self, other: &mut IntrusiveList<T>) {
        if other.is_empty() {
            return;
        }

        if self.is_empty() {
            self.head = other.head;
            self.tail = other.tail;
            self.len = other.len;
        } else {
            unsafe {
                if let (Some(mut self_tail), Some(mut other_head)) = (self.tail, other.head) {
                    self_tail.as_mut().next = Some(other_head);
                    other_head.as_mut().prev = Some(self_tail);
                }
                self.tail = other.tail;
                self.len += other.len;
            }
        }

        other.head = None;
        other.tail = None;
        other.len = 0;
    }

    /// Split the list at the given node, returning the second half
    ///
    /// # Safety
    /// Node must be in this list
    pub unsafe fn split_off(&mut self, at: NonNull<IntrusiveNode<T>>) -> IntrusiveList<T> {
        let at_ref = at.as_ref();
        let at_ptr = at.as_ptr();

        let new_list_len = if let Some(prev) = at_ref.prev {
            // Count nodes before split point
            let mut count = 1;
            let mut curr = Some(prev);
            while let Some(node) = curr {
                count += 1;
                curr = node.as_ref().prev;
            }
            count
        } else {
            0
        };

        let split_len = self.len - new_list_len;

        // Update previous node to point to None
        if let Some(mut prev) = at_ref.prev {
            prev.as_mut().next = None;
            self.tail = Some(prev);
        } else {
            self.head = None;
            self.tail = None;
        }

        // Clear prev pointer of split node
        (*at_ptr).prev = None;

        self.len = new_list_len;

        IntrusiveList {
            head: Some(at),
            tail: self.tail.filter(|_| split_len == 1).or(self.tail),
            len: split_len,
        }
    }

    /// Splice nodes from other list at the given position
    ///
    /// # Safety
    /// - `at` must be in this list, or None to splice at end
    /// - `other` must be a valid list
    pub unsafe fn splice(
        &mut self,
        at: Option<NonNull<IntrusiveNode<T>>>,
        other: &mut IntrusiveList<T>,
    ) {
        if other.is_empty() {
            return;
        }

        let (other_head, other_tail) = match (other.head, other.tail) {
            (Some(h), Some(t)) => (h, t),
            _ => return,
        };

        let other_len = other.len;

        if let Some(at_node) = at {
            let at_ref = at_node.as_ref();
            let at_ptr = at_node.as_ptr();

            // Insert before `at`
            if let Some(mut prev) = at_ref.prev {
                prev.as_mut().next = Some(other_head);
                other_head.as_ptr().cast::<IntrusiveNode<T>>().write(IntrusiveNode {
                    prev: Some(prev),
                    next: other_head.as_ref().next,
                    _marker: PhantomData,
                });
            } else {
                // Splicing at head
                self.head = Some(other_head);
                (*other_head.as_ptr()).prev = None;
            }

            (*other_tail.as_ptr()).next = Some(at_node);
            (*at_ptr).prev = Some(other_tail);
        } else {
            // Splice at end
            if let Some(mut tail) = self.tail {
                tail.as_mut().next = Some(other_head);
                (*other_head.as_ptr()).prev = Some(tail);
                self.tail = Some(other_tail);
            } else {
                self.head = Some(other_head);
                self.tail = Some(other_tail);
            }
        }

        self.len += other_len;

        other.head = None;
        other.tail = None;
        other.len = 0;
    }
}

impl<T> Default for IntrusiveList<T> {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<T: Send> Send for IntrusiveList<T> {}
unsafe impl<T: Sync> Sync for IntrusiveList<T> {}

/// Immutable iterator over intrusive list
pub struct Iter<'a, T> {
    head: Option<NonNull<IntrusiveNode<T>>>,
    tail: Option<NonNull<IntrusiveNode<T>>>,
    len: usize,
    _marker: PhantomData<&'a IntrusiveNode<T>>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a IntrusiveNode<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        self.head.map(|node| unsafe {
            self.len -= 1;
            let node_ref = node.as_ref();
            self.head = node_ref.next;
            node_ref
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl<'a, T> DoubleEndedIterator for Iter<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        self.tail.map(|node| unsafe {
            self.len -= 1;
            let node_ref = node.as_ref();
            self.tail = node_ref.prev;
            node_ref
        })
    }
}

impl<'a, T> ExactSizeIterator for Iter<'a, T> {
    fn len(&self) -> usize {
        self.len
    }
}

/// Mutable iterator over intrusive list
pub struct IterMut<'a, T> {
    head: Option<NonNull<IntrusiveNode<T>>>,
    tail: Option<NonNull<IntrusiveNode<T>>>,
    len: usize,
    _marker: PhantomData<&'a mut IntrusiveNode<T>>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut IntrusiveNode<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        self.head.map(|mut node| unsafe {
            self.len -= 1;
            let node_ref = node.as_mut();
            self.head = node_ref.next;
            node_ref
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl<'a, T> DoubleEndedIterator for IterMut<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        self.tail.map(|mut node| unsafe {
            self.len -= 1;
            let node_ref = node.as_mut();
            self.tail = node_ref.prev;
            node_ref
        })
    }
}

impl<'a, T> ExactSizeIterator for IterMut<'a, T> {
    fn len(&self) -> usize {
        self.len
    }
}

/// Cursor for navigating intrusive list
pub struct Cursor<'a, T> {
    current: Option<NonNull<IntrusiveNode<T>>>,
    list: &'a IntrusiveList<T>,
}

impl<'a, T> Cursor<'a, T> {
    /// Get current node
    pub fn current(&self) -> Option<&'a IntrusiveNode<T>> {
        self.current.map(|node| unsafe { node.as_ref() })
    }

    /// Move to next node
    pub fn move_next(&mut self) -> bool {
        if let Some(current) = self.current {
            unsafe {
                self.current = current.as_ref().next;
                self.current.is_some()
            }
        } else {
            false
        }
    }

    /// Move to previous node
    pub fn move_prev(&mut self) -> bool {
        if let Some(current) = self.current {
            unsafe {
                self.current = current.as_ref().prev;
                self.current.is_some()
            }
        } else {
            false
        }
    }

    /// Peek at next node without moving
    pub fn peek_next(&self) -> Option<&'a IntrusiveNode<T>> {
        self.current
            .and_then(|node| unsafe { node.as_ref().next })
            .map(|node| unsafe { node.as_ref() })
    }

    /// Peek at previous node without moving
    pub fn peek_prev(&self) -> Option<&'a IntrusiveNode<T>> {
        self.current
            .and_then(|node| unsafe { node.as_ref().prev })
            .map(|node| unsafe { node.as_ref() })
    }

    /// Reset cursor to front
    pub fn reset_to_front(&mut self) {
        self.current = self.list.head;
    }

    /// Reset cursor to back
    pub fn reset_to_back(&mut self) {
        self.current = self.list.tail;
    }

    /// Check if at end (current is None)
    pub fn is_at_end(&self) -> bool {
        self.current.is_none()
    }
}

/// Mutable cursor for navigating and modifying intrusive list
pub struct CursorMut<'a, T> {
    current: Option<NonNull<IntrusiveNode<T>>>,
    list: &'a mut IntrusiveList<T>,
}

impl<'a, T> CursorMut<'a, T> {
    /// Get current node
    pub fn current(&self) -> Option<&IntrusiveNode<T>> {
        self.current.map(|node| unsafe { node.as_ref() })
    }

    /// Get current node mutably
    pub fn current_mut(&mut self) -> Option<&mut IntrusiveNode<T>> {
        self.current.map(|mut node| unsafe { node.as_mut() })
    }

    /// Move to next node
    pub fn move_next(&mut self) -> bool {
        if let Some(current) = self.current {
            unsafe {
                self.current = current.as_ref().next;
                self.current.is_some()
            }
        } else {
            false
        }
    }

    /// Move to previous node
    pub fn move_prev(&mut self) -> bool {
        if let Some(current) = self.current {
            unsafe {
                self.current = current.as_ref().prev;
                self.current.is_some()
            }
        } else {
            false
        }
    }

    /// Peek at next node without moving
    pub fn peek_next(&self) -> Option<&IntrusiveNode<T>> {
        self.current
            .and_then(|node| unsafe { node.as_ref().next })
            .map(|node| unsafe { node.as_ref() })
    }

    /// Peek at previous node without moving
    pub fn peek_prev(&self) -> Option<&IntrusiveNode<T>> {
        self.current
            .and_then(|node| unsafe { node.as_ref().prev })
            .map(|node| unsafe { node.as_ref() })
    }

    /// Reset cursor to front
    pub fn reset_to_front(&mut self) {
        self.current = self.list.head;
    }

    /// Reset cursor to back
    pub fn reset_to_back(&mut self) {
        self.current = self.list.tail;
    }

    /// Check if at end (current is None)
    pub fn is_at_end(&self) -> bool {
        self.current.is_none()
    }

    /// Insert node before current position
    ///
    /// # Safety
    /// - `node` must be part of a valid `T` object
    /// - Node must not already be in a list
    pub unsafe fn insert_before(&mut self, node: NonNull<IntrusiveNode<T>>) {
        let node_ptr = node.as_ptr();
        (*node_ptr).init();

        if let Some(current) = self.current {
            let current_ref = current.as_ref();

            (*node_ptr).next = Some(current);
            (*node_ptr).prev = current_ref.prev;

            if let Some(mut prev) = current_ref.prev {
                prev.as_mut().next = Some(node);
            } else {
                self.list.head = Some(node);
            }

            (*current.as_ptr()).prev = Some(node);
            self.list.len += 1;
        } else {
            // At end, insert at back
            self.list.push_back(node);
        }
    }

    /// Insert node after current position
    ///
    /// # Safety
    /// Same as insert_before
    pub unsafe fn insert_after(&mut self, node: NonNull<IntrusiveNode<T>>) {
        let node_ptr = node.as_ptr();
        (*node_ptr).init();

        if let Some(current) = self.current {
            let current_ref = current.as_ref();

            (*node_ptr).prev = Some(current);
            (*node_ptr).next = current_ref.next;

            if let Some(mut next) = current_ref.next {
                next.as_mut().prev = Some(node);
            } else {
                self.list.tail = Some(node);
            }

            (*current.as_ptr()).next = Some(node);
            self.list.len += 1;
        } else {
            // At end, insert at back
            self.list.push_back(node);
        }
    }

    /// Remove current node and move to next
    ///
    /// Returns the removed node
    pub fn remove_current(&mut self) -> Option<NonNull<IntrusiveNode<T>>> {
        let current = self.current?;

        unsafe {
            let next = current.as_ref().next;
            self.list.remove(current);
            self.current = next;
            Some(current)
        }
    }

    /// Split the list at current position, returning the second half
    ///
    /// Current becomes the head of the new list
    pub fn split_at_current(&mut self) -> IntrusiveList<T> {
        if let Some(current) = self.current {
            unsafe { self.list.split_off(current) }
        } else {
            IntrusiveList::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestItem {
        value: i32,
        node: IntrusiveNode<TestItem>,
    }

    impl TestItem {
        fn new(value: i32) -> Self {
            Self {
                value,
                node: IntrusiveNode::new(),
            }
        }

        fn node_ptr(&mut self) -> NonNull<IntrusiveNode<TestItem>> {
            unsafe { NonNull::new_unchecked(&mut self.node) }
        }
    }

    #[test]
    fn test_intrusive_list() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);
        let mut item3 = TestItem::new(3);

        assert!(list.is_empty());

        unsafe {
            list.push_back(item1.node_ptr());
            list.push_back(item2.node_ptr());
            list.push_back(item3.node_ptr());
        }

        assert_eq!(list.len(), 3);

        let node = list.pop_front().unwrap();
        assert_eq!(list.len(), 2);

        let node2 = list.pop_back().unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_intrusive_list_push_front() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);

        unsafe {
            list.push_front(item2.node_ptr());
            list.push_front(item1.node_ptr());
        }

        assert_eq!(list.len(), 2);
        assert_eq!(list.front(), Some(item1.node_ptr()));
        assert_eq!(list.back(), Some(item2.node_ptr()));
    }

    #[test]
    fn test_intrusive_list_remove() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);
        let mut item3 = TestItem::new(3);

        unsafe {
            list.push_back(item1.node_ptr());
            list.push_back(item2.node_ptr());
            list.push_back(item3.node_ptr());

            list.remove(item2.node_ptr());
        }

        assert_eq!(list.len(), 2);
        assert_eq!(list.front(), Some(item1.node_ptr()));
        assert_eq!(list.back(), Some(item3.node_ptr()));
    }

    #[test]
    fn test_intrusive_list_iter() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);
        let mut item3 = TestItem::new(3);

        unsafe {
            list.push_back(item1.node_ptr());
            list.push_back(item2.node_ptr());
            list.push_back(item3.node_ptr());
        }

        let count = list.iter().count();
        assert_eq!(count, 3);

        let vec: Vec<_> = list.iter().collect();
        assert_eq!(vec.len(), 3);
    }

    #[test]
    fn test_intrusive_list_iter_double_ended() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);
        let mut item3 = TestItem::new(3);

        unsafe {
            list.push_back(item1.node_ptr());
            list.push_back(item2.node_ptr());
            list.push_back(item3.node_ptr());
        }

        let mut iter = list.iter();
        assert!(iter.next().is_some());
        assert!(iter.next_back().is_some());
        assert_eq!(iter.len(), 1);
    }

    #[test]
    fn test_intrusive_list_iter_mut() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);
        let mut item3 = TestItem::new(3);

        unsafe {
            list.push_back(item1.node_ptr());
            list.push_back(item2.node_ptr());
            list.push_back(item3.node_ptr());
        }

        let count = list.iter_mut().count();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_intrusive_list_cursor() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);
        let mut item3 = TestItem::new(3);

        unsafe {
            list.push_back(item1.node_ptr());
            list.push_back(item2.node_ptr());
            list.push_back(item3.node_ptr());
        }

        let mut cursor = list.cursor_front();
        assert!(cursor.current().is_some());
        assert!(cursor.move_next());
        assert!(cursor.peek_prev().is_some());
        assert!(cursor.peek_next().is_some());
        assert!(cursor.move_next());
        assert!(!cursor.move_next());
        assert!(cursor.is_at_end());
    }

    #[test]
    fn test_intrusive_list_cursor_mut() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);

        unsafe {
            list.push_back(item1.node_ptr());
            list.push_back(item2.node_ptr());
        }

        let mut cursor = list.cursor_front_mut();
        assert!(cursor.current().is_some());
        assert!(cursor.current_mut().is_some());
        assert!(cursor.move_next());
        assert!(!cursor.move_next());
    }

    #[test]
    fn test_intrusive_list_cursor_insert() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);
        let mut item3 = TestItem::new(3);

        unsafe {
            list.push_back(item1.node_ptr());
            list.push_back(item3.node_ptr());
        }

        assert_eq!(list.len(), 2);

        let mut cursor = list.cursor_front_mut();
        cursor.move_next();

        unsafe {
            cursor.insert_before(item2.node_ptr());
        }

        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_intrusive_list_cursor_remove() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);
        let mut item3 = TestItem::new(3);

        unsafe {
            list.push_back(item1.node_ptr());
            list.push_back(item2.node_ptr());
            list.push_back(item3.node_ptr());
        }

        let mut cursor = list.cursor_front_mut();
        cursor.move_next();
        let removed = cursor.remove_current();

        assert!(removed.is_some());
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_intrusive_list_append() {
        let mut list1 = IntrusiveList::<TestItem>::new();
        let mut list2 = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);
        let mut item3 = TestItem::new(3);
        let mut item4 = TestItem::new(4);

        unsafe {
            list1.push_back(item1.node_ptr());
            list1.push_back(item2.node_ptr());
            list2.push_back(item3.node_ptr());
            list2.push_back(item4.node_ptr());
        }

        list1.append(&mut list2);

        assert_eq!(list1.len(), 4);
        assert_eq!(list2.len(), 0);
        assert!(list2.is_empty());
    }

    #[test]
    fn test_intrusive_list_split_off() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);
        let mut item3 = TestItem::new(3);
        let mut item4 = TestItem::new(4);

        unsafe {
            list.push_back(item1.node_ptr());
            list.push_back(item2.node_ptr());
            list.push_back(item3.node_ptr());
            list.push_back(item4.node_ptr());
        }

        let list2 = unsafe { list.split_off(item3.node_ptr()) };

        assert_eq!(list.len(), 2);
        assert_eq!(list2.len(), 2);
    }

    #[test]
    fn test_intrusive_list_cursor_split() {
        let mut list = IntrusiveList::<TestItem>::new();
        let mut item1 = TestItem::new(1);
        let mut item2 = TestItem::new(2);
        let mut item3 = TestItem::new(3);

        unsafe {
            list.push_back(item1.node_ptr());
            list.push_back(item2.node_ptr());
            list.push_back(item3.node_ptr());
        }

        let mut cursor = list.cursor_front_mut();
        cursor.move_next();

        let list2 = cursor.split_at_current();

        assert_eq!(list.len(), 1);
        assert_eq!(list2.len(), 2);
    }
}
