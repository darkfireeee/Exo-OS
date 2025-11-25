//! C-style string handling
//! 
//! Provides safe wrappers for null-terminated C strings

use core::str;
use core::slice;
use core::fmt;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;

/// A borrowed C string (null-terminated)
#[derive(PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct CStr {
    inner: [u8],
}

impl CStr {
    /// Create a CStr from a pointer (unsafe - must be valid null-terminated string)
    /// 
    /// # Safety
    /// - ptr must point to a valid null-terminated C string
    /// - The string must outlive the returned CStr
    pub unsafe fn from_ptr<'a>(ptr: *const u8) -> &'a Self {
        let len = strlen(ptr);
        let slice = slice::from_raw_parts(ptr, len);
        Self::from_bytes_with_nul_unchecked(slice)
    }
    
    /// Create CStr from byte slice with null terminator
    pub fn from_bytes_with_nul(bytes: &[u8]) -> Result<&Self, CStrError> {
        if bytes.is_empty() {
            return Err(CStrError::NotNulTerminated);
        }
        
        // Check for null terminator at end
        if *bytes.last().unwrap() != 0 {
            return Err(CStrError::NotNulTerminated);
        }
        
        // Check for interior nulls
        if bytes[..bytes.len() - 1].contains(&0) {
            return Err(CStrError::InteriorNull);
        }
        
        Ok(unsafe { Self::from_bytes_with_nul_unchecked(bytes) })
    }
    
    /// Create CStr from byte slice without validation (unsafe)
    pub unsafe fn from_bytes_with_nul_unchecked(bytes: &[u8]) -> &Self {
        &*(bytes as *const [u8] as *const CStr)
    }
    
    /// Get as byte slice (without null terminator)
    pub fn to_bytes(&self) -> &[u8] {
        let len = self.inner.len();
        if len > 0 && self.inner[len - 1] == 0 {
            &self.inner[..len - 1]
        } else {
            &self.inner
        }
    }
    
    /// Get as byte slice (with null terminator)
    pub fn to_bytes_with_nul(&self) -> &[u8] {
        &self.inner
    }
    
    /// Convert to string slice
    pub fn to_str(&self) -> Result<&str, str::Utf8Error> {
        str::from_utf8(self.to_bytes())
    }
    
    /// Get as raw pointer
    pub fn as_ptr(&self) -> *const u8 {
        self.inner.as_ptr()
    }
    
    /// Get length (without null terminator)
    pub fn len(&self) -> usize {
        self.to_bytes().len()
    }
    
    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Debug for CStr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "\"")?;
        for &byte in self.to_bytes() {
            for ch in core::ascii::escape_default(byte) {
                write!(f, "{}", ch as char)?;
            }
        }
        write!(f, "\"")
    }
}

impl fmt::Display for CStr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.to_str() {
            Ok(s) => write!(f, "{}", s),
            Err(_) => write!(f, "{:?}", self.to_bytes()),
        }
    }
}

/// Owned C string
pub struct CString {
    inner: Vec<u8>,
}

impl CString {
    /// Create new CString from byte vector
    pub fn new(bytes: Vec<u8>) -> Result<Self, CStrError> {
        // Check for interior nulls
        if bytes.contains(&0) {
            return Err(CStrError::InteriorNull);
        }
        
        let mut inner = bytes;
        inner.push(0); // Add null terminator
        
        Ok(Self { inner })
    }
    
    /// Create CString from string
    pub fn from_str(s: &str) -> Result<Self, CStrError> {
        Self::new(s.as_bytes().to_vec())
    }
    
    /// Get as CStr
    pub fn as_c_str(&self) -> &CStr {
        unsafe { CStr::from_bytes_with_nul_unchecked(&self.inner) }
    }
    
    /// Get as raw pointer
    pub fn as_ptr(&self) -> *const u8 {
        self.inner.as_ptr()
    }
    
    /// Convert into raw pointer (caller must free)
    pub fn into_raw(self) -> *mut u8 {
        let ptr = self.inner.as_ptr() as *mut u8;
        core::mem::forget(self);
        ptr
    }
    
    /// Reconstruct CString from raw pointer
    /// 
    /// # Safety
    /// ptr must have been created by into_raw
    pub unsafe fn from_raw(ptr: *mut u8) -> Self {
        let len = strlen(ptr);
        let vec = Vec::from_raw_parts(ptr, len + 1, len + 1);
        Self { inner: vec }
    }
}

impl fmt::Debug for CString {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_c_str().fmt(f)
    }
}

impl fmt::Display for CString {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_c_str().fmt(f)
    }
}

impl Clone for CString {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// C string errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CStrError {
    NotNulTerminated,
    InteriorNull,
}

/// Calculate strlen (unsafe - for C interop)
unsafe fn strlen(ptr: *const u8) -> usize {
    let mut len = 0;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    len
}

/// C-compatible strlen function
#[no_mangle]
pub unsafe extern "C" fn strlen_ffi(s: *const u8) -> usize {
    strlen(s)
}

/// C-compatible strcmp function
#[no_mangle]
pub unsafe extern "C" fn strcmp(s1: *const u8, s2: *const u8) -> i32 {
    let mut i = 0;
    loop {
        let c1 = *s1.add(i);
        let c2 = *s2.add(i);
        
        if c1 != c2 {
            return c1 as i32 - c2 as i32;
        }
        
        if c1 == 0 {
            return 0;
        }
        
        i += 1;
    }
}

/// C-compatible strncpy function
#[no_mangle]
pub unsafe extern "C" fn strncpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    let mut i = 0;
    while i < n {
        let c = *src.add(i);
        *dest.add(i) = c;
        if c == 0 {
            break;
        }
        i += 1;
    }
    
    // Fill remaining with nulls
    while i < n {
        *dest.add(i) = 0;
        i += 1;
    }
    
    dest
}
