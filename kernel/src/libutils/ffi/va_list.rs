//! Support des listes d'arguments variables
//! 
//! Ce module fournit des types et des fonctions pour travailler avec des listes
//! d'arguments variables, similaires à va_list en C.

use core::marker::PhantomData;
use core::mem;

/// Représente une liste d'arguments variables
#[repr(transparent)]
pub struct VaList<'a> {
    inner: VaListImpl<'a>,
}

/// Implémentation interne de VaList
#[repr(C)]
struct VaListImpl<'a> {
    stack_top: *const u8,
    stack_bottom: *const u8,
    reg_save_area: *const u8,
    gp_offset: usize,
    fp_offset: usize,
    overflow_arg_area: *const u8,
    reg_save_area_end: *const u8,
    _marker: PhantomData<&'a ()>,
}

impl<'a> VaList<'a> {
    /// Crée une nouvelle VaList vide
    pub const fn new() -> Self {
        Self {
            inner: VaListImpl {
                stack_top: core::ptr::null(),
                stack_bottom: core::ptr::null(),
                reg_save_area: core::ptr::null(),
                gp_offset: 0,
                fp_offset: 0,
                overflow_arg_area: core::ptr::null(),
                reg_save_area_end: core::ptr::null(),
                _marker: PhantomData,
            },
        }
    }
    
    /// Initialise la VaList à partir des registres et de la pile
    /// 
    /// # Safety
    /// Cette fonction est unsafe car elle manipule directement des pointeurs
    pub unsafe fn from_registers(
        stack_top: *const u8,
        stack_bottom: *const u8,
        reg_save_area: *const u8,
        gp_offset: usize,
        fp_offset: usize,
    ) -> Self {
        let overflow_arg_area = stack_top;
        let reg_save_area_end = reg_save_area.add(48); // 6 registres GP + 8 registres FP * 8 octets
        
        Self {
            inner: VaListImpl {
                stack_top,
                stack_bottom,
                reg_save_area,
                gp_offset,
                fp_offset,
                overflow_arg_area,
                reg_save_area_end,
                _marker: PhantomData,
            },
        }
    }
    
    /// Extrait le prochain argument de type T
    /// 
    /// # Safety
    /// Cette fonction est unsafe car elle lit des données potentiellement non initialisées
    pub unsafe fn arg<T>(&mut self) -> T {
        let size = mem::size_of::<T>();
        let align = mem::align_of::<T>();
        
        // Vérifier si l'argument peut être passé dans un registre
        if size <= 8 && align <= 8 {
            // Argument entier ou pointeur
            if self.inner.gp_offset < 48 {
                // Passé dans un registre GP
                let ptr = self.inner.reg_save_area.add(self.inner.gp_offset);
                self.inner.gp_offset += 8;
                return ptr.cast::<T>().read_unaligned();
            }
        } else if size <= 16 && align <= 16 {
            // Argument flottant ou SIMD
            if self.inner.fp_offset < 176 {
                // Passé dans un registre FP
                let ptr = self.inner.reg_save_area.add(self.inner.fp_offset);
                self.inner.fp_offset += 16;
                return ptr.cast::<T>().read_unaligned();
            }
        }
        
        // Argument passé sur la pile
        let mut addr = self.inner.overflow_arg_area as usize;
        
        // Aligner l'adresse
        if addr % align != 0 {
            addr = (addr + align - 1) & !(align - 1);
        }
        
        let ptr = addr as *const T;
        self.inner.overflow_arg_area = ptr.add(1) as *const u8;
        
        ptr.read_unaligned()
    }
    
    /// Extrait le prochain argument de type i32
    pub fn i32(&mut self) -> i32 {
        unsafe { self.arg::<i32>() }
    }
    
    /// Extrait le prochain argument de type u32
    pub fn u32(&mut self) -> u32 {
        unsafe { self.arg::<u32>() }
    }
    
    /// Extrait le prochain argument de type i64
    pub fn i64(&mut self) -> i64 {
        unsafe { self.arg::<i64>() }
    }
    
    /// Extrait le prochain argument de type u64
    pub fn u64(&mut self) -> u64 {
        unsafe { self.arg::<u64>() }
    }
    
    /// Extrait le prochain argument de type isize
    pub fn isize(&mut self) -> isize {
        unsafe { self.arg::<isize>() }
    }
    
    /// Extrait le prochain argument de type usize
    pub fn usize(&mut self) -> usize {
        unsafe { self.arg::<usize>() }
    }
    
    /// Extrait le prochain argument de type *const T
    pub fn ptr<T>(&mut self) -> *const T {
        unsafe { self.arg::<*const T>() }
    }
    
    /// Extrait le prochain argument de type *mut T
    pub fn ptr_mut<T>(&mut self) -> *mut T {
        unsafe { self.arg::<*mut T>() }
    }
    
    /// Extrait le prochain argument de type f64
    pub fn f64(&mut self) -> f64 {
        unsafe { self.arg::<f64>() }
    }
}

impl<'a> Clone for VaList<'a> {
    fn clone(&self) -> Self {
        Self {
            inner: VaListImpl {
                stack_top: self.inner.stack_top,
                stack_bottom: self.inner.stack_bottom,
                reg_save_area: self.inner.reg_save_area,
                gp_offset: self.inner.gp_offset,
                fp_offset: self.inner.fp_offset,
                overflow_arg_area: self.inner.overflow_arg_area,
                reg_save_area_end: self.inner.reg_save_area_end,
                _marker: PhantomData,
            },
        }
    }
}

impl<'a> Copy for VaList<'a> {}

/// Macro pour créer une fonction qui accepte une liste d'arguments variables
#[macro_export]
macro_rules! va_func {
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($($param:ident: $ptype:ty),*) -> $ret:ty {
            $($body:tt)*
        }
    ) => {
        $(#[$meta])*
        $vis unsafe extern "C" fn $name($($param: $ptype), ...) -> $ret {
            // Créer une VaList à partir des registres et de la pile
            let mut args = $crate::ffi::va_list::VaList::new();
            
            // Appeler la fonction interne avec la VaList
            $name_impl($($param),*, args)
        }
        
        // Fonction interne qui prend une VaList
        $vis fn $name_impl($($param: $ptype),*, mut args: $crate::ffi::va_list::VaList) -> $ret {
            $($body)*
        }
    };
}