//! Implémentation simple de vecteur pour no_std
//! 
//! Ce module fournit une implémentation de base de vecteur qui peut être
//! utilisée dans un environnement de noyau sans allocation dynamique.

use core::ptr;
use core::mem;
use core::ops::{Index, IndexMut, Deref, DerefMut};

/// Vecteur simple pour environnement no_std
pub struct Vec<T> {
    ptr: *mut T,
    len: usize,
    capacity: usize,
}

impl<T> Vec<T> {
    /// Crée un nouveau vecteur vide
    pub const fn new() -> Self {
        Vec {
            ptr: ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }
    
    /// Crée un vecteur avec une capacité initiale
    pub fn with_capacity(capacity: usize) -> Self {
        // Dans un vrai noyau, nous utiliserions un allocateur personnalisé ici
        // Pour cet exemple, nous utilisons un allocateur global
        let layout = alloc::alloc::Layout::array::<T>(capacity)
            .expect("Capacité trop grande ou alignement incorrect");
        
        let ptr = unsafe { alloc::alloc::alloc(layout) as *mut T };
        
        Vec {
            ptr,
            len: 0,
            capacity,
        }
    }
    
    /// Retourne le nombre d'éléments dans le vecteur
    pub fn len(&self) -> usize {
        self.len
    }
    
    /// Retourne true si le vecteur ne contient aucun élément
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    
    /// Retourne la capacité du vecteur
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    
    /// Ajoute un élément à la fin du vecteur
    pub fn push(&mut self, item: T) {
        if self.len == self.capacity {
            self.grow();
        }
        
        unsafe {
            ptr::write(self.ptr.add(self.len), item);
        }
        self.len += 1;
    }
    
    /// Retire et retourne le dernier élément
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        
        self.len -= 1;
        Some(unsafe { ptr::read(self.ptr.add(self.len)) })
    }
    
    /// Augmente la capacité du vecteur
    fn grow(&mut self) {
        let new_capacity = if self.capacity == 0 {
            4
        } else {
            self.capacity * 2
        };
        
        let new_layout = alloc::alloc::Layout::array::<T>(new_capacity)
            .expect("Capacité trop grande ou alignement incorrect");
        
        let new_ptr = if self.capacity == 0 {
            unsafe { alloc::alloc::alloc(new_layout) as *mut T }
        } else {
            let old_layout = alloc::alloc::Layout::array::<T>(self.capacity)
                .expect("Capacité trop grande ou alignement incorrect");
            
            unsafe {
                alloc::alloc::realloc(
                    self.ptr as *mut u8,
                    old_layout,
                    new_layout.size(),
                ) as *mut T
            }
        };
        
        self.ptr = new_ptr;
        self.capacity = new_capacity;
    }
    
    /// Vide le vecteur
    pub fn clear(&mut self) {
        // Destructeur pour tous les éléments
        for i in 0..self.len {
            unsafe {
                ptr::drop_in_place(self.ptr.add(i));
            }
        }
        
        self.len = 0;
    }
}

impl<T> Drop for Vec<T> {
    fn drop(&mut self) {
        self.clear();
        
        if self.capacity > 0 {
            let layout = alloc::alloc::Layout::array::<T>(self.capacity)
                .expect("Capacité trop grande ou alignement incorrect");
            
            unsafe {
                alloc::alloc::dealloc(self.ptr as *mut u8, layout);
            }
        }
    }
}

impl<T> Index<usize> for Vec<T> {
    type Output = T;
    
    fn index(&self, index: usize) -> &Self::Output {
        assert!(index < self.len, "Index hors limites");
        unsafe { &*self.ptr.add(index) }
    }
}

impl<T> IndexMut<usize> for Vec<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        assert!(index < self.len, "Index hors limites");
        unsafe { &mut *self.ptr.add(index) }
    }
}

impl<T> Deref for Vec<T> {
    type Target = [T];
    
    fn deref(&self) -> &Self::Target {
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl<T> DerefMut for Vec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}