//! HTree Directory Indexing
//!
//! Hash tree directory indexing for fast lookups in large directories.
//! Provides O(1) average lookup time vs O(n) for linear scan.

use crate::fs::{FsError, FsResult};
use alloc::vec::Vec;
use alloc::string::String;

/// Hash algorithm
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    Legacy = 0,
    HalfMD4 = 1,
    Tea = 2,
    LegacyUnsigned = 3,
    HalfMD4Unsigned = 4,
    TeaUnsigned = 5,
}

impl HashAlgorithm {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(HashAlgorithm::Legacy),
            1 => Some(HashAlgorithm::HalfMD4),
            2 => Some(HashAlgorithm::Tea),
            3 => Some(HashAlgorithm::LegacyUnsigned),
            4 => Some(HashAlgorithm::HalfMD4Unsigned),
            5 => Some(HashAlgorithm::TeaUnsigned),
            _ => None,
        }
    }
}

/// Directory hash
#[derive(Debug, Clone, Copy)]
pub struct DirHash {
    pub hash: u32,
    pub minor_hash: u32,
}

/// HTree root
#[derive(Debug)]
pub struct HTreeRoot {
    pub algorithm: HashAlgorithm,
    pub seed: [u32; 4],
    pub entries: Vec<HTreeEntry>,
}

/// HTree index entry
#[derive(Debug, Clone)]
pub struct HTreeEntry {
    pub hash: u32,
    pub block: u32,
}

/// HTree implementation
pub struct HTree {
    algorithm: HashAlgorithm,
    seed: [u32; 4],
}

impl HTree {
    /// Create new HTree
    pub fn new(algorithm: HashAlgorithm, seed: [u32; 4]) -> Self {
        Self { algorithm, seed }
    }

    /// Hash a filename
    pub fn hash_name(&self, name: &str) -> DirHash {
        match self.algorithm {
            HashAlgorithm::Tea | HashAlgorithm::TeaUnsigned => self.tea_hash(name),
            HashAlgorithm::HalfMD4 | HashAlgorithm::HalfMD4Unsigned => self.half_md4_hash(name),
            _ => self.legacy_hash(name),
        }
    }

    /// TEA hash (Tiny Encryption Algorithm)
    fn tea_hash(&self, name: &str) -> DirHash {
        let mut hash = self.seed[0];
        let mut minor_hash = self.seed[1];

        let bytes = name.as_bytes();
        let mut buf = [0u32; 4];

        for chunk in bytes.chunks(16) {
            // Convert to u32 words
            for (i, word_bytes) in chunk.chunks(4).enumerate() {
                if i < 4 {
                    let mut word = 0u32;
                    for (j, &byte) in word_bytes.iter().enumerate() {
                        word |= (byte as u32) << (j * 8);
                    }
                    buf[i] = word;
                }
            }

            // TEA rounds
            self.tea_transform(&mut hash, &mut minor_hash, &buf);
        }

        DirHash { hash, minor_hash }
    }

    /// TEA transformation
    fn tea_transform(&self, hash: &mut u32, minor_hash: &mut u32, buf: &[u32; 4]) {
        const DELTA: u32 = 0x9E3779B9;
        let mut sum = 0u32;

        let mut a = *hash;
        let mut b = *minor_hash;

        for _ in 0..16 {
            sum = sum.wrapping_add(DELTA);
            a = a.wrapping_add(
                ((b << 4).wrapping_add(buf[0])) ^ (b.wrapping_add(sum)) ^ ((b >> 5).wrapping_add(buf[1]))
            );
            b = b.wrapping_add(
                ((a << 4).wrapping_add(buf[2])) ^ (a.wrapping_add(sum)) ^ ((a >> 5).wrapping_add(buf[3]))
            );
        }

        *hash = a;
        *minor_hash = b;
    }

    /// Half-MD4 hash (simplified)
    fn half_md4_hash(&self, name: &str) -> DirHash {
        let mut hash = self.seed[0];
        let mut minor_hash = self.seed[1];

        for &byte in name.as_bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(byte as u32);
            minor_hash = minor_hash.wrapping_mul(37).wrapping_add(byte as u32);
        }

        DirHash { hash, minor_hash }
    }

    /// Legacy hash
    fn legacy_hash(&self, name: &str) -> DirHash {
        let mut hash = self.seed[0];
        let mut minor_hash = self.seed[1];

        for &byte in name.as_bytes() {
            hash = hash.wrapping_mul(0x8001).wrapping_add((byte as u32) << 8);
            minor_hash = minor_hash.wrapping_add(byte as u32);
        }

        DirHash { hash, minor_hash }
    }

    /// Lookup block for hash value
    pub fn lookup_block(&self, root: &HTreeRoot, hash: u32) -> Option<u32> {
        // Binary search in sorted entries
        let mut low = 0;
        let mut high = root.entries.len();

        while low < high {
            let mid = (low + high) / 2;

            if root.entries[mid].hash <= hash {
                low = mid + 1;
            } else {
                high = mid;
            }
        }

        if low > 0 {
            Some(root.entries[low - 1].block)
        } else {
            None
        }
    }

    /// Build HTree from entries
    pub fn build_tree(&self, entries: &[(String, u64)]) -> FsResult<HTreeRoot> {
        // Hash all entries
        let mut hashed: Vec<(DirHash, String, u64)> = entries
            .iter()
            .map(|(name, ino)| (self.hash_name(name), name.clone(), *ino))
            .collect();

        // Sort by hash
        hashed.sort_by_key(|(hash, _, _)| hash.hash);

        // Build index (simplified - would create actual block structure)
        let mut htree_entries = Vec::new();
        let entries_per_block = 512; // Simplified

        for (i, chunk) in hashed.chunks(entries_per_block).enumerate() {
            if let Some((hash, _, _)) = chunk.first() {
                htree_entries.push(HTreeEntry {
                    hash: hash.hash,
                    block: i as u32,
                });
            }
        }

        Ok(HTreeRoot {
            algorithm: self.algorithm,
            seed: self.seed,
            entries: htree_entries,
        })
    }
}
