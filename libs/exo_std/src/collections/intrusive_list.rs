<<<<<<< Updated upstream
// libs/exo_std/src/collections/intrusive_list.rs
//! Intrusive doubly-linked list
=======
//! Liste intrusive pour structures a zero allocation
//!
//! Les noeuds sont integres dans les structures parent.
>>>>>>> Stashed changes

use core::ptr::NonNull;
use core::marker::PhantomData;

<<<<<<< Updated upstream
/// Node for intrusive linked list
#[repr(C)]
pub struct IntrusiveNode {
    next: Option<NonNull<IntrusiveNode>>,
    prev: Option<NonNull<IntrusiveNode>>,
}

impl IntrusiveNode {
    /// Create new unlinked node
=======
/// Nœud de liste intrusive
pub struct IntrusiveNode<T> {
    next: Option<NonNull<IntrusiveNode<T>>>,
    prev: Option<NonNull<IntrusiveNode<T>>>,
    _marker: PhantomData<T>,
}

impl<T> IntrusiveNode<T> {
    /// Crée un nouveau nœud
>>>>>>> Stashed changes
    pub const fn new() -> Self {
        Self {
            next: None,
            prev: None,
<<<<<<< Updated upstream
        }
    }
    
    /// Check if node is linked
=======
            _marker: PhantomData,
        }
    }

    /// Initialise le nœud
    pub fn init(&mut self) {
        self.next = None;
        self.prev = None;
    }

    /// Vérifie si le nœud est lié
>>>>>>> Stashed changes
    pub fn is_linked(&self) -> bool {
        self.next.is_some() || self.prev.is_some()
    }
}

<<<<<<< Updated upstream
/// Intrusive doubly-linked list
pub struct IntrusiveList<T> {
    head: Option<NonNull<IntrusiveNode>>,
    tail: Option<NonNull<IntrusiveNode>>,
    len: usize,
    _phantom: PhantomData<T>,
}

impl<T> IntrusiveList<T> {
    /// Create new empty list
=======
/// Liste intrusive
pub struct IntrusiveList<T> {
    head: Option<NonNull<IntrusiveNode<T>>>,
    tail: Option<NonNull<IntrusiveNode<T>>>,
    len: usize,
}

impl<T> IntrusiveList<T> {
    /// Crée une nouvelle liste vide
>>>>>>> Stashed changes
    pub const fn new() -> Self {
        Self {
            head: None,
            tail: None,
            len: 0,
<<<<<<< Updated upstream
            _phantom: PhantomData,
        }
    }
    
    /// Push front (O(1))
    ///
    /// # Safety
    /// - `node` must be part of a valid `T` object
    /// - Node must not already be in a list
    /// - Object containing node must outlive the list
    pub unsafe fn push_front(&mut self, node: NonNull<IntrusiveNode>) {
        let node_ref = node.as_ref();
        debug_assert!(!node_ref.is_linked());
        
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
    pub unsafe fn push_back(&mut self, node: NonNull<IntrusiveNode>) {
        let node_ref = node.as_ref();
        debug_assert!(!node_ref.is_linked());
        
        let node_ptr = node.as_ptr();
        
        (*node_ptr).prev = self.tail;
        (*node_ptr).next = None;
        
=======
        }
    }

    /// Vérifie si la liste est vide
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Retourne la longueur
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Ajoute un nœud à la fin
    pub unsafe fn push_back(&mut self, node: NonNull<IntrusiveNode<T>>) {
        let node_ptr = node.as_ptr();
        (*node_ptr).next = None;
        (*node_ptr).prev = self.tail;

>>>>>>> Stashed changes
        if let Some(mut tail) = self.tail {
            tail.as_mut().next = Some(node);
        } else {
            self.head = Some(node);
        }
<<<<<<< Updated upstream
        
        self.tail = Some(node);
        self.len += 1;
    }
    
    /// Pop front (O(1))
    pub fn pop_front(&mut self) -> Option<NonNull<IntrusiveNode>> {
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
    pub fn pop_back(&mut self) -> Option<NonNull<IntrusiveNode>> {
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
    pub unsafe fn remove(&mut self, node: NonNull<IntrusiveNode>) {
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
    pub fn front(&self) -> Option<NonNull<IntrusiveNode>> {
        self.head
    }
    
    /// Get back node
    pub fn back(&self) -> Option<NonNull<IntrusiveNode>> {
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
}

unsafe impl<T: Send> Send for IntrusiveList<T> {}
unsafe impl<T: Sync> Sync for IntrusiveList<T> {}
=======

        self.tail = Some(node);
        self.len += 1;
    }

    /// Ajoute un nœud au début
    pub unsafe fn push_front(&mut self, node: NonNull<IntrusiveNode<T>>) {
        let node_ptr = node.as_ptr();
        (*node_ptr).prev = None;
        (*node_ptr).next = self.head;

        if let Some(mut head) = self.head {
            head.as_mut().prev = Some(node);
        } else {
            self.tail = Some(node);
        }

        self.head = Some(node);
        self.len += 1;
    }

    /// Retire le premier nœud
    pub unsafe fn pop_front(&mut self) -> Option<NonNull<IntrusiveNode<T>>> {
        let head = self.head?;
        let head_ptr = head.as_ptr();

        self.head = (*head_ptr).next;

        if let Some(mut new_head) = self.head {
            new_head.as_mut().prev = None;
        } else {
            self.tail = None;
        }

        (*head_ptr).next = None;
        (*head_ptr).prev = None;
        self.len -= 1;

        Some(head)
    }

    /// Retire le dernier nœud
    pub unsafe fn pop_back(&mut self) -> Option<NonNull<IntrusiveNode<T>>> {
        let tail = self.tail?;
        let tail_ptr = tail.as_ptr();

        self.tail = (*tail_ptr).prev;

        if let Some(mut new_tail) = self.tail {
            new_tail.as_mut().next = None;
        } else {
            self.head = None;
        }

        (*tail_ptr).next = None;
        (*tail_ptr).prev = None;
        self.len -= 1;

        Some(tail)
    }

    /// Retire un nœud spécifique
    pub unsafe fn remove(&mut self, node: NonNull<IntrusiveNode<T>>) {
        let node_ptr = node.as_ptr();

        if let Some(mut prev) = (*node_ptr).prev {
            prev.as_mut().next = (*node_ptr).next;
        } else {
            self.head = (*node_ptr).next;
        }

        if let Some(mut next) = (*node_ptr).next {
            next.as_mut().prev = (*node_ptr).prev;
        } else {
            self.tail = (*node_ptr).prev;
        }

        (*node_ptr).next = None;
        (*node_ptr).prev = None;
        self.len -= 1;
    }

    /// Supprime tous les nœuds
    pub fn clear(&mut self) {
        unsafe {
            while self.pop_front().is_some() {}
        }
    }
}

impl<T> Default for IntrusiveList<T> {
    fn default() -> Self {
        Self::new()
    }
}
>>>>>>> Stashed changes

#[cfg(test)]
mod tests {
    use super::*;
<<<<<<< Updated upstream
    
    struct TestItem {
        value: i32,
        node: IntrusiveNode,
    }
    
    impl TestItem {
        fn new(value: i32) -> Self {
            Self {
                value,
                node: IntrusiveNode::new(),
            }
        }
        
        fn node_ptr(&mut self) -> NonNull<IntrusiveNode> {
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
        assert_eq!(unsafe { &*(node.as_ptr() as *const TestItem) }.value, 1);
        
        assert_eq!(list.len(), 2);
=======

    struct TestNode {
        value: i32,
        link: IntrusiveNode<TestNode>,
    }

    #[test]
    fn test_intrusive_list() {
        let mut list = IntrusiveList::new();
        assert!(list.is_empty());

        let mut node1 = TestNode {
            value: 1,
            link: IntrusiveNode::new(),
        };

        let mut node2 = TestNode {
            value: 2,
            link: IntrusiveNode::new(),
        };

        unsafe {
            list.push_back(NonNull::from(&mut node1.link));
            list.push_back(NonNull::from(&mut node2.link));
        }

        assert_eq!(list.len(), 2);

        unsafe {
            let first = list.pop_front().unwrap();
            assert_eq!(first.as_ptr(), &mut node1.link as *mut _);
        }

        assert_eq!(list.len(), 1);
>>>>>>> Stashed changes
    }
}
