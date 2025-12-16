//! FAT32 Long Filename (LFN) Support
//!
//! Support complet des long filenames avec UTF-16

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

/// LFN Entry (Long Filename Entry)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct LfnEntry {
    pub order: u8,              // Ordre (1-20), bit 6 = last entry
    pub name1: [u16; 5],        // Caractères 1-5
    pub attr: u8,               // 0x0F (LFN marker)
    pub entry_type: u8,         // 0 (reserved)
    pub checksum: u8,           // Checksum du short name
    pub name2: [u16; 6],        // Caractères 6-11
    pub first_cluster_low: u16, // 0 (reserved)
    pub name3: [u16; 2],        // Caractères 12-13
}

impl LfnEntry {
    /// Est-ce le dernier entry de la séquence?
    #[inline(always)]
    pub fn is_last(&self) -> bool {
        (self.order & 0x40) != 0
    }
    
    /// Récupère le numéro d'ordre
    #[inline(always)]
    pub fn sequence_number(&self) -> u8 {
        self.order & 0x1F
    }
    
    /// Est-ce un LFN entry valide?
    #[inline(always)]
    pub fn is_lfn(&self) -> bool {
        self.attr == 0x0F
    }
    
    /// Extrait les 13 caractères UTF-16
    pub fn extract_chars(&self) -> [u16; 13] {
        let mut chars = [0u16; 13];
        // Copier depuis packed struct via copy locale
        let name1 = self.name1;
        let name2 = self.name2;
        let name3 = self.name3;
        chars[0..5].copy_from_slice(&name1);
        chars[5..11].copy_from_slice(&name2);
        chars[11..13].copy_from_slice(&name3);
        chars
    }
    
    /// Calcule le checksum d'un short name 8.3
    pub fn calculate_checksum(short_name: &[u8; 11]) -> u8 {
        let mut sum = 0u8;
        for &byte in short_name.iter() {
            sum = ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(byte);
        }
        sum
    }
}

/// Parseur de Long Filename
pub struct LfnParser {
    /// Accumule les LFN entries
    entries: Vec<LfnEntry>,
    /// Checksum attendu
    expected_checksum: u8,
}

impl LfnParser {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            expected_checksum: 0,
        }
    }
    
    /// Ajoute un LFN entry
    ///
    /// Returns true si la séquence est complète
    pub fn push_entry(&mut self, entry: LfnEntry) -> bool {
        if entry.is_last() {
            self.entries.clear();
            self.expected_checksum = entry.checksum;
        }
        
        self.entries.push(entry);
        
        // Check si on a tous les entries (ordre 1 à N)
        if !self.entries.is_empty() {
            let first = self.entries.last().unwrap();
            first.sequence_number() == 1
        } else {
            false
        }
    }
    
    /// Construit le nom complet depuis les LFN entries
    pub fn build_name(&self, short_name_checksum: u8) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        
        // Vérifie checksum
        if short_name_checksum != self.expected_checksum {
            return None;
        }
        
        let mut utf16_chars = Vec::new();
        
        // Parcours en ordre inverse (last → first)
        for entry in self.entries.iter().rev() {
            let chars = entry.extract_chars();
            for &c in &chars {
                if c == 0 || c == 0xFFFF {
                    break;
                }
                utf16_chars.push(c);
            }
        }
        
        // Convertit UTF-16 → String
        String::from_utf16(&utf16_chars).ok()
    }
    
    /// Reset le parser
    pub fn reset(&mut self) {
        self.entries.clear();
    }
}

impl Default for LfnParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Encodeur de Long Filename (pour write support)
pub struct LfnEncoder;

impl LfnEncoder {
    /// Encode un long filename en séquence de LFN entries
    pub fn encode(name: &str, short_name_checksum: u8) -> Vec<LfnEntry> {
        // Convertit String → UTF-16
        let utf16: Vec<u16> = name.encode_utf16().collect();
        
        // Calcule nombre d'entries nécessaires (13 chars par entry)
        let entry_count = (utf16.len() + 12) / 13;
        
        let mut entries = Vec::new();
        
        for i in 0..entry_count {
            let start = i * 13;
            let end = (start + 13).min(utf16.len());
            let mut chars = [0xFFFFu16; 13];
            
            for (j, &c) in utf16[start..end].iter().enumerate() {
                chars[j] = c;
            }
            
            // Padding avec 0 puis 0xFFFF
            if end < start + 13 {
                chars[end - start] = 0;
            }
            
            let order = if i == entry_count - 1 {
                (entry_count as u8) | 0x40 // Last entry
            } else {
                entry_count as u8 - i as u8
            };
            
            let mut name1 = [0u16; 5];
            let mut name2 = [0u16; 6];
            let mut name3 = [0u16; 2];
            
            name1.copy_from_slice(&chars[0..5]);
            name2.copy_from_slice(&chars[5..11]);
            name3.copy_from_slice(&chars[11..13]);
            
            entries.push(LfnEntry {
                order,
                name1,
                attr: 0x0F,
                entry_type: 0,
                checksum: short_name_checksum,
                name2,
                first_cluster_low: 0,
                name3,
            });
        }
        
        entries
    }
    
    /// Génère un short name 8.3 depuis un long name
    pub fn generate_short_name(name: &str, sequence: u32) -> [u8; 11] {
        let mut short = [b' '; 11];
        
        // Extrait basename et extension
        let (base, ext) = if let Some(dot_pos) = name.rfind('.') {
            (&name[..dot_pos], &name[dot_pos + 1..])
        } else {
            (name, "")
        };
        
        // Base name (max 8 chars)
        let base_clean: String = base.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(6)
            .collect::<String>()
            .to_uppercase();
        
        for (i, byte) in base_clean.bytes().enumerate() {
            short[i] = byte;
        }
        
        // Add ~N pour unicité
        let seq_str = format!("~{}", sequence);
        let seq_bytes = seq_str.as_bytes();
        let base_len = base_clean.len().min(6);
        short[base_len..base_len + seq_bytes.len()].copy_from_slice(seq_bytes);
        
        // Extension (max 3 chars)
        if !ext.is_empty() {
            let ext_clean: String = ext.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(3)
                .collect::<String>()
                .to_uppercase();
            
            for (i, byte) in ext_clean.bytes().enumerate() {
                short[8 + i] = byte;
            }
        }
        
        short
    }
}
