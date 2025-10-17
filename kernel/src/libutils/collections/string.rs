//! Gestion des chaînes de caractères pour no_std
//! 
//! Ce module fournit une implémentation de chaîne de caractères
//! adaptée à un environnement de noyau.

use core::fmt;
use core::ops::{Deref, DerefMut};

/// Chaîne de caractères pour environnement no_std
pub struct String {
    vec: Vec<u8>,
}

impl String {
    /// Crée une nouvelle chaîne vide
    pub const fn new() -> Self {
        String {
            vec: Vec::new(),
        }
    }
    
    /// Crée une chaîne à partir d'une chaîne statique
    pub fn from(s: &'static str) -> Self {
        let mut string = String::new();
        string.push_str(s);
        string
    }
    
    /// Ajoute une chaîne à la fin
    pub fn push_str(&mut self, s: &str) {
        for byte in s.bytes() {
            self.vec.push(byte);
        }
    }
    
    /// Ajoute un caractère à la fin
    pub fn push(&mut self, c: char) {
        let mut buf = [0; 4];
        let s = c.encode_utf8(&mut buf);
        self.push_str(s);
    }
    
    /// Retourne la longueur de la chaîne
    pub fn len(&self) -> usize {
        self.vec.len()
    }
    
    /// Retourne true si la chaîne est vide
    pub fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }
    
    /// Vide la chaîne
    pub fn clear(&mut self) {
        self.vec.clear();
    }
}

impl Deref for String {
    type Target = str;
    
    fn deref(&self) -> &Self::Target {
        // Sécurité: nous nous assurons que le vecteur contient de l'UTF-8 valide
        // Dans une implémentation réelle, nous devrions valider l'UTF-8
        unsafe { core::str::from_utf8_unchecked(&self.vec) }
    }
}

impl DerefMut for String {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Sécurité: nous nous assurons que le vecteur contient de l'UTF-8 valide
        // Dans une implémentation réelle, nous devrions valider l'UTF-8
        unsafe { core::str::from_utf8_unchecked_mut(&mut self.vec) }
    }
}

impl fmt::Display for String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &self[..])
    }
}

impl From<&str> for String {
    fn from(s: &str) -> Self {
        let mut string = String::new();
        string.push_str(s);
        string
    }
}