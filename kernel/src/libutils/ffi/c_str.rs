//! Gestion des chaînes C
//! 
//! Ce module fournit des types et des fonctions pour travailler avec des chaînes
//! de caractères au format C (terminées par un byte nul).

use core::fmt;
use core::ops::Deref;
use core::slice;
use core::str;

/// Représente une chaîne de caractères C
#[repr(transparent)]
pub struct CStr {
    inner: [u8],
}

impl CStr {
    /// Crée une CStr à partir d'un pointeur vers une chaîne C
    /// 
    /// # Safety
    /// Le pointeur doit pointer vers une chaîne C valide (terminée par un byte nul)
    pub unsafe fn from_ptr<'a>(ptr: *const u8) -> &'a CStr {
        if ptr.is_null() {
            panic!("CStr::from_ptr: pointeur nul");
        }
        
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        
        let slice = slice::from_raw_parts(ptr, len);
        Self::from_bytes_with_nul_unchecked(slice)
    }
    
    /// Crée une CStr à partir d'un slice d'octets avec un byte nul à la fin
    /// 
    /// # Safety
    /// Le slice doit contenir un byte nul et ne pas contenir de byte nul interne
    pub unsafe fn from_bytes_with_nul_unchecked(bytes: &[u8]) -> &CStr {
        &*(bytes as *const [u8] as *const CStr)
    }
    
    /// Crée une CStr à partir d'un slice d'octets avec un byte nul à la fin
    pub fn from_bytes_with_nul(bytes: &[u8]) -> Result<&CStr, FromBytesWithNulError> {
        if bytes.is_empty() {
            return Err(FromBytesWithNulError::MissingNul);
        }
        
        let nul_pos = match bytes.iter().position(|&b| b == 0) {
            Some(pos) => pos,
            None => return Err(FromBytesWithNulError::MissingNul),
        };
        
        if nul_pos + 1 != bytes.len() {
            return Err(FromBytesWithNulError::InteriorNul(nul_pos));
        }
        
        unsafe { Ok(Self::from_bytes_with_nul_unchecked(bytes)) }
    }
    
    /// Retourne un slice des octets de la chaîne (sans le byte nul)
    pub fn as_bytes(&self) -> &[u8] {
        &self.inner
    }
    
    /// Retourne un slice des octets de la chaîne avec le byte nul
    pub fn as_bytes_with_nul(&self) -> &[u8] {
        let len = self.inner.len() + 1;
        unsafe {
            let ptr = self.inner.as_ptr();
            slice::from_raw_parts(ptr, len)
        }
    }
    
    /// Convertit la chaîne C en une chaîne Rust si elle contient de l'UTF-8 valide
    pub fn to_str(&self) -> Result<&str, str::Utf8Error> {
        str::from_utf8(self.as_bytes())
    }
    
    /// Convertit la chaîne C en une chaîne Rust, en remplaçant les octets UTF-8 invalides
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(self.as_bytes()).into_owned()
    }
    
    /// Retourne un pointeur vers la chaîne C
    pub fn as_ptr(&self) -> *const u8 {
        self.inner.as_ptr()
    }
}

impl Deref for CStr {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl fmt::Debug for CStr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "\"{}\"", self.to_string_lossy())
    }
}

impl fmt::Display for CStr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_string_lossy())
    }
}

/// Erreur retournée lors de la conversion d'un slice en CStr
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromBytesWithNulError {
    /// Le slice ne contient pas de byte nul
    MissingNul,
    /// Le slice contient un byte nul à une position interne
    InteriorNul(usize),
}

impl fmt::Display for FromBytesWithNulError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FromBytesWithNulError::MissingNul => {
                write!(f, "données fournies ne contiennent pas de byte nul à la fin")
            }
            FromBytesWithNulError::InteriorNul(pos) => {
                write!(f, "données fournies contiennent un byte nul à la position {}", pos)
            }
        }
    }
}

/// Crée une chaîne C à partir d'un littéral de chaîne Rust
#[macro_export]
macro_rules! cstr {
    ($s:expr) => {
        unsafe {
            $crate::ffi::c_str::CStr::from_bytes_with_nul_unchecked(
                concat!($s, "\0").as_bytes()
            )
        }
    };
}