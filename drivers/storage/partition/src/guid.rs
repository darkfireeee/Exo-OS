//! GUID (mixed-endian "on-disk" GPT layout).
//!
//! Sur disque, un GUID GPT est stocké en **mixed-endian** (RFC 4122 / Microsoft) :
//! - data1 (u32) little-endian,
//! - data2 (u16) little-endian,
//! - data3 (u16) little-endian,
//! - data4 (8 octets) tels quels.
//!
//! On stocke donc les 16 octets **tels qu'ils apparaissent sur le disque** et on
//! compare octet à octet. `parse_str` convertit la forme canonique
//! `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX` vers cette disposition on-disk.

use core::fmt;

/// GUID 16 octets dans la disposition **on-disk** (mixed-endian GPT).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Guid(pub [u8; 16]);

const HYPHENATED_LEN: usize = 36;

#[inline]
const fn hex(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'A'..=b'F' => b - b'A' + 10,
        b'a'..=b'f' => b - b'a' + 10,
        _ => panic!("chiffre hexadécimal invalide dans le GUID"),
    }
}

impl Guid {
    pub const NIL: Self = Self([0u8; 16]);

    /// Vrai si tous les octets sont nuls (entrée de partition vide).
    #[inline]
    pub fn is_nil(&self) -> bool {
        self.0 == [0u8; 16]
    }

    /// Parse une forme canonique `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX` vers la
    /// disposition **on-disk** (data1/2/3 little-endian, data4 brut). `const`.
    pub const fn parse_str(s: &str) -> Self {
        let b = s.as_bytes();
        if b.len() != HYPHENATED_LEN {
            panic!("longueur de GUID invalide");
        }
        if b[8] != b'-' || b[13] != b'-' || b[18] != b'-' || b[23] != b'-' {
            panic!("format de GUID invalide");
        }
        // Décode les 32 chiffres hex en 16 octets "big-endian canonique".
        let mut raw = [0u8; 16];
        let mut i = 0; // index dans la chaîne
        let mut j = 0; // index octet
        while i < HYPHENATED_LEN {
            if b[i] == b'-' {
                i += 1;
                continue;
            }
            raw[j] = (hex(b[i]) << 4) | hex(b[i + 1]);
            i += 2;
            j += 1;
        }
        // Réordonne vers la disposition on-disk :
        // data1 (raw[0..4]) -> LE, data2 (raw[4..6]) -> LE, data3 (raw[6..8]) -> LE,
        // data4 (raw[8..16]) -> brut.
        let out = [
            raw[3], raw[2], raw[1], raw[0], // data1 LE
            raw[5], raw[4], // data2 LE
            raw[7], raw[6], // data3 LE
            raw[8], raw[9], raw[10], raw[11], raw[12], raw[13], raw[14], raw[15], // data4
        ];
        Self(out)
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let d = &self.0;
        write!(
            f,
            "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
            // data1 : octets on-disk LE → affiché big-endian (inversé)
            d[3], d[2], d[1], d[0],
            d[5], d[4],
            d[7], d[6],
            d[8], d[9],
            d[10], d[11], d[12], d[13], d[14], d[15],
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Guid({})", self)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Type-GUIDs connus
// ─────────────────────────────────────────────────────────────────────────────

/// EFI System Partition (standard UEFI).
pub const GUID_ESP: Guid = Guid::parse_str("C12A7328-F81F-11D2-BA4B-00A0C93EC93B");

/// Partition ExoFS ROOT (type custom ExoOS — cf. SPEC-BOOTLOADER-GPT-STRATA).
/// Octets on-disk fixés par le schéma ExoOS Strata.
pub const GUID_EXOOS_ROOT: Guid = Guid([
    0x52, 0x4F, 0x58, 0x45, 0x4F, 0x4F, 0x00, 0x53, 0x52, 0x4F, 0x4F, 0x54, 0x00, 0x00, 0x00, 0x02,
]);

/// Partition ExoFS DATA (type custom ExoOS).
pub const GUID_EXOOS_DATA: Guid = Guid([
    0x52, 0x4F, 0x58, 0x45, 0x4F, 0x4F, 0x00, 0x53, 0x44, 0x41, 0x54, 0x41, 0x00, 0x00, 0x00, 0x03,
]);

/// Catégorie de partition reconnue (type-GUID résolu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionType {
    Esp,
    ExoFsRoot,
    ExoFsData,
    /// Type GUID non reconnu (conservé pour inspection).
    Other,
}

impl Guid {
    /// Résout le type-GUID en catégorie ExoOS connue.
    pub fn partition_type(&self) -> PartitionType {
        if *self == GUID_ESP {
            PartitionType::Esp
        } else if *self == GUID_EXOOS_ROOT {
            PartitionType::ExoFsRoot
        } else if *self == GUID_EXOOS_DATA {
            PartitionType::ExoFsData
        } else {
            PartitionType::Other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_str_produces_ondisk_layout() {
        // ESP canonique → octets on-disk corrects (data1 LE = 28 73 2A C1).
        let esp = Guid::parse_str("C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
        assert_eq!(
            esp.0,
            [0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B]
        );
        assert_eq!(esp, GUID_ESP);
    }

    #[test]
    fn display_roundtrips_canonical() {
        let esp = GUID_ESP;
        // Display ressort la forme canonique.
        extern crate std;
        assert_eq!(std::format!("{esp}"), "C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
    }

    #[test]
    fn type_resolution() {
        assert_eq!(GUID_ESP.partition_type(), PartitionType::Esp);
        assert_eq!(GUID_EXOOS_ROOT.partition_type(), PartitionType::ExoFsRoot);
        assert_eq!(GUID_EXOOS_DATA.partition_type(), PartitionType::ExoFsData);
        assert_eq!(Guid::NIL.partition_type(), PartitionType::Other);
        assert!(Guid::NIL.is_nil());
    }
}
