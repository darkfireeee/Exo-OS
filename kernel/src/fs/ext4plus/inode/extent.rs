// kernel/src/fs/ext4plus/inode/extent.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ EXTENT TREE — arbre B à 4 niveaux + REFLINKS CoW  (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// L'extent tree est un arbre B interne à l'inode (12 × u32 = 60 octets dans
// i_block[]).  Il remplace le mapping indirect classique.
// Niveaux : 0 = feuilles (leaf), 1-3 = index (index nodes).
//
// Structures on-disk :
//   Ext4ExtentHeader  (12 octets)      — en-tête de nœud
//   Ext4ExtentIdx     (12 octets)      — nœud interne
//   Ext4ExtentLeaf    (12 octets)      — feuille (mapping logique → physique)
//
// REFLINKS (RÈGLE FS-EXT4P-04) :
//   cp --reflink src dst → nouvel inode avec MÊME extent tree, aucune copie disque.
//   Écriture dans dst → cow_if_shared() appelé AVANT write → CoW ciblé sur le
//   seul bloc modifié. Les autres blocs restent partagés (refcount inchangé).
//
//   Compteur de références :
//     • TABLE_REFCOUNT : BTreeMap<u64, AtomicU32> (bloc physique → refcount)
//     • refcount > 1  → CoW obligatoire avant écriture
//     • refcount == 1 → écriture directe (plus de partage)
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::collections::BTreeMap;

use crate::fs::core::types::{FsError, FsResult};
use crate::fs::block::bio::{Bio, BioOp, BioFlags, BioVec};
use crate::fs::block::queue::submit_bio;
use crate::memory::core::types::PhysAddr;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// TABLE_REFCOUNT — compteur de références par bloc physique (reflinks CoW)
// ─────────────────────────────────────────────────────────────────────────────
//
// RÈGLE FS-EXT4P-04 : chaque bloc physique partagé entre plusieurs inodes
// (via reflink) doit avoir un refcount ≥ 2. Lorsque refcount > 1, toute
// écriture DOIT passer par cow_if_shared() pour obtenir un bloc exclusif.

pub struct RefcountTable {
    inner: SpinLock<BTreeMap<u64, u32>>,
}

impl RefcountTable {
    pub const fn new() -> Self { Self { inner: SpinLock::new(BTreeMap::new()) } }

    /// Incrémente le refcount d'un bloc physique (lors d'un reflink).
    pub fn inc(&self, phys_block: u64) {
        let mut map = self.inner.lock();
        let entry = map.entry(phys_block).or_insert(1);
        *entry += 1;
        EXTENT_STATS.refcount_incs.fetch_add(1, Ordering::Relaxed);
    }

    /// Décrémente. Retourne true si le refcount tombe à 0 (bloc libérable).
    pub fn dec(&self, phys_block: u64) -> bool {
        let mut map = self.inner.lock();
        if let Some(rc) = map.get_mut(&phys_block) {
            if *rc > 1 {
                *rc -= 1;
                EXTENT_STATS.refcount_decs.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            map.remove(&phys_block);
            EXTENT_STATS.refcount_decs.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        true // absent = uniquement référencé, traitez-le comme libérable
    }

    /// Retourne le refcount actuel d'un bloc (1 si absent = référence unique).
    pub fn get(&self, phys_block: u64) -> u32 {
        self.inner.lock().get(&phys_block).copied().unwrap_or(1)
    }
}

pub static TABLE_REFCOUNT: RefcountTable = RefcountTable::new();

// ─────────────────────────────────────────────────────────────────────────────
// ext4_reflink_extent — crée un reflink (two inodes, same physical blocks)
// ─────────────────────────────────────────────────────────────────────────────

/// Crée un reflink : le nouvel inode `dst_i_block` reçoit une copie
/// de l'extent tree source, et chaque bloc physique voit son refcount incrémenté.
///
/// Aucune donnée n'est copiée sur disque — la copie est instantanée.
/// CoW ne se produit qu'au moment de la première modification (cow_if_shared).
pub fn ext4_reflink_extent(
    src_i_block: &[u8; 60],
    dst_i_block: &mut [u8; 60],
) -> FsResult<()> {
    // Copie l'arbre inline (60 octets suffisent pour un reflink simple).
    dst_i_block.copy_from_slice(src_i_block);

    // SAFETY: i_block_data contient 60 octets initialisés, lus depuis le SB.
    let root = unsafe { ExtentNode::from_raw(src_i_block.as_ptr(), 60)? };
    // Incrémente le refcount de toutes les feuilles.
    for leaf in root.leaves.iter() {
        let phys = leaf.phys_block();
        for off in 0..leaf.len() as u64 {
            TABLE_REFCOUNT.inc(phys + off);
        }
    }
    EXTENT_STATS.reflinks.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// cow_if_shared — CoW ciblé sur un bloc partagé (RÈGLE FS-EXT4P-04)
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie si `phys_block` est partagé (refcount > 1).
/// Si oui, alloue un nouveau bloc, copie les données, décrémente le refcount
/// de l'ancien bloc, et retourne le numéro du nouveau bloc exclusif.
///
/// Doit être appelé AVANT toute écriture dans un extent potentiellement partagé.
///
/// `dev`, `block_size`, `copy_buf` : nécessaires pour lire l'ancien bloc.
/// `alloc_fn` : fonction d'allocation (pointe vers balloc::ext4_alloc_block).
pub fn cow_if_shared(
    phys_block: u64,
    dev:        u64,
    block_size: u64,
    read_buf:   PhysAddr,
    write_buf:  PhysAddr,
    alloc_fn:   impl Fn() -> FsResult<u64>,
) -> FsResult<u64> {
    if TABLE_REFCOUNT.get(phys_block) <= 1 {
        // Bloc exclusif — pas de CoW nécessaire.
        return Ok(phys_block);
    }

    // ── Étape 1 : Lire l'ancien bloc
    let sector_old = phys_block * block_size / 512;
    let bio_read = Bio {
        id:       phys_block,
        op:       BioOp::Read,
        dev,
        sector:   sector_old,
        vecs:     alloc::vec![BioVec { phys: read_buf, virt: read_buf.as_u64(), len: block_size as u32, offset: 0 }],
        flags:    BioFlags::empty(),
        status:   core::sync::atomic::AtomicU8::new(0),
        bytes:    core::sync::atomic::AtomicU64::new(0),
        callback: None,
        cb_data:  0,
    };
    submit_bio(bio_read)?;

    // ── Étape 2 : Allouer un nouveau bloc exclusif
    let new_block = alloc_fn()?;

    // ── Étape 3 : Copier les données dans le nouveau bloc
    // SAFETY: read_buf et write_buf sont des adresses physiques valides de block_size octets.
    unsafe {
        core::ptr::copy_nonoverlapping(
            read_buf.as_u64()  as *const u8,
            write_buf.as_u64() as *mut   u8,
            block_size as usize,
        );
    }
    let sector_new = new_block * block_size / 512;
    let bio_write = Bio {
        id:       new_block,
        op:       BioOp::Write,
        dev,
        sector:   sector_new,
        vecs:     alloc::vec![BioVec { phys: write_buf, virt: write_buf.as_u64(), len: block_size as u32, offset: 0 }],
        flags:    BioFlags::FUA,
        status:   core::sync::atomic::AtomicU8::new(0),
        bytes:    core::sync::atomic::AtomicU64::new(0),
        callback: None,
        cb_data:  0,
    };
    submit_bio(bio_write)?;

    // ── Étape 4 : Décrémenter le refcount de l'ANCIEN bloc
    TABLE_REFCOUNT.dec(phys_block);

    EXTENT_STATS.cow_performed.fetch_add(1, Ordering::Relaxed);
    Ok(new_block)
}

// ─────────────────────────────────────────────────────────────────────────────
// Structures on-disk
// ─────────────────────────────────────────────────────────────────────────────

pub const EXT4_EXT_MAGIC: u16 = 0xF30A;
pub const EXT4_EXT_MAX_DEPTH: u16 = 5;

/// En-tête d'un nœud de l'arbre (header).
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct Ext4ExtentHeader {
    pub eh_magic:      u16,  // 0xF30A
    pub eh_entries:    u16,  // nb d'entrées valides
    pub eh_max:        u16,  // nb d'entrées max dans ce nœud
    pub eh_depth:      u16,  // 0 = feuille, ≥1 = index
    pub eh_generation: u32,
}

/// Nœud index : pointe vers un bloc contenant des feuilles ou d'autres index.
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct Ext4ExtentIdx {
    pub ei_block:   u32,     // premier bloc logique couvert
    pub ei_leaf_lo: u32,     // bloc physique (lo)
    pub ei_leaf_hi: u16,     // bloc physique (hi, 64-bit)
    pub ei_unused:  u16,
}

/// Feuille : mapping blocs logiques → blocs physiques.
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct Ext4ExtentLeaf {
    pub ee_block:    u32,    // premier bloc logique
    pub ee_len:      u16,    // nb de blocs (≤ 32768) ; bit15=initialized
    pub ee_start_hi: u16,    // bloc physique hi (64-bit)
    pub ee_start_lo: u32,    // bloc physique lo
}

impl core::fmt::Debug for Ext4ExtentLeaf {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Lecture des champs packed via copie pour éviter les UB d'alignement.
        let block = self.ee_block;
        let len   = self.ee_len;
        let hi    = self.ee_start_hi;
        let lo    = self.ee_start_lo;
        f.debug_struct("Ext4ExtentLeaf")
            .field("ee_block",    &block)
            .field("ee_len",      &len)
            .field("ee_start_hi", &hi)
            .field("ee_start_lo", &lo)
            .finish()
    }
}

impl Ext4ExtentLeaf {
    pub fn is_initialized(&self) -> bool { self.ee_len & 0x8000 == 0 }
    pub fn len(&self) -> u16            { self.ee_len & 0x7FFF }
    pub fn phys_block(&self) -> u64     { self.ee_start_lo as u64 | ((self.ee_start_hi as u64) << 32) }
    pub fn logical_block(&self) -> u32  { self.ee_block }
}

impl Ext4ExtentIdx {
    pub fn leaf_block(&self) -> u64 { self.ei_leaf_lo as u64 | ((self.ei_leaf_hi as u64) << 32) }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExtentNode — nœud chargé en mémoire
// ─────────────────────────────────────────────────────────────────────────────

pub struct ExtentNode {
    pub header:  Ext4ExtentHeader,
    /// Feuilles (si depth == 0)
    pub leaves:  Vec<Ext4ExtentLeaf>,
    /// Index (si depth > 0)
    pub indexes: Vec<Ext4ExtentIdx>,
}

impl ExtentNode {
    /// Charge un nœud depuis un tampon de données brutes (block_size octets).
    ///
    /// # Safety
    /// `data` doit pointer sur au moins `size` octets initialisés.
    unsafe fn from_raw(data: *const u8, _size: usize) -> FsResult<Self> {
        let hdr = (data as *const Ext4ExtentHeader).read_unaligned();
        if hdr.eh_magic != EXT4_EXT_MAGIC { return Err(FsError::Corrupt); }
        let count = hdr.eh_entries as usize;
        if hdr.eh_depth == 0 {
            let leaf_ptr = data.add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4ExtentLeaf;
            let leaves   = (0..count).map(|i| leaf_ptr.add(i).read_unaligned()).collect();
            Ok(Self { header: hdr, leaves, indexes: Vec::new() })
        } else {
            let idx_ptr = data.add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4ExtentIdx;
            let indexes = (0..count).map(|i| idx_ptr.add(i).read_unaligned()).collect();
            Ok(Self { header: hdr, leaves: Vec::new(), indexes })
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ext4_find_extent — recherche un bloc logique dans l'arbre
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une recherche dans l'extent tree.
#[derive(Clone, Debug)]
pub struct ExtentResult {
    pub leaf:         Ext4ExtentLeaf,
    /// Offset dans l'extent (block_idx - leaf.ee_block)
    pub offset_in_ext: u32,
}

/// Cherche le bloc physique correspondant à `logical_block`.
///
/// `i_block_data` : les 60 octets de i_block[] déjà chargés.
/// `dev` et `block_size` pour charger des blocs intermédiaires.
pub fn ext4_find_extent(
    i_block_data: &[u8; 60],
    logical_block: u32,
    dev: u64,
    block_size: u64,
    load_buf: PhysAddr,
) -> FsResult<Option<ExtentResult>> {
    // SAFETY: i_block_data est 60 octets alignés, initialisés par le disque.
    let root = unsafe { ExtentNode::from_raw(i_block_data.as_ptr(), 60)? };
    find_in_node(&root, logical_block, dev, block_size, load_buf)
}

fn find_in_node(
    node:   &ExtentNode,
    lblock: u32,
    dev:    u64,
    bsize:  u64,
    buf:    PhysAddr,
) -> FsResult<Option<ExtentResult>> {
    if node.header.eh_depth == 0 {
        // Feuille — recherche binaire
        let mut lo = 0i32;
        let mut hi = node.leaves.len() as i32 - 1;
        while lo <= hi {
            let mid = ((lo + hi) / 2) as usize;
            let l   = &node.leaves[mid];
            if lblock < l.ee_block {
                hi = mid as i32 - 1;
            } else if lblock >= l.ee_block + l.len() as u32 {
                lo = mid as i32 + 1;
            } else {
                return Ok(Some(ExtentResult {
                    leaf:          *l,
                    offset_in_ext: lblock - l.ee_block,
                }));
            }
        }
        return Ok(None);
    }
    // Nœud index — trouve l'index couvrant lblock
    let indexes = &node.indexes;
    let mut chosen: Option<&Ext4ExtentIdx> = None;
    for idx in indexes.iter() {
        if idx.ei_block <= lblock { chosen = Some(idx); } else { break; }
    }
    let idx = chosen.ok_or(FsError::Corrupt)?;
    // Charge le bloc correspondant
    let sector = idx.leaf_block() * bsize / 512;
    let bio = Bio {
        id:       0,
        op:       BioOp::Read,
        dev,
        sector,
        vecs:     alloc::vec![BioVec { phys: buf, virt: buf.as_u64(), len: bsize as u32, offset: 0 }],
        flags:    BioFlags::META,
        status:   core::sync::atomic::AtomicU8::new(0),
        bytes:    core::sync::atomic::AtomicU64::new(0),
        callback: None,
        cb_data:  0,
    };
    submit_bio(bio)?;
    // SAFETY: buffer rempli par le BIO ci-dessus, ≥ bsize octets initialisés.
    let child = unsafe { ExtentNode::from_raw(buf.as_u64() as *const u8, bsize as usize)? };
    EXTENT_STATS.index_traversals.fetch_add(1, Ordering::Relaxed);
    find_in_node(&child, lblock, dev, bsize, buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// ext4_insert_extent — insère une nouvelle feuille (inline dans i_block)
// ─────────────────────────────────────────────────────────────────────────────

/// Insère une feuille dans le nœud racine (64-byte inline tree, ≤4 feuilles).
/// Retourne une erreur si arbre plein ou si un nœud split est nécessaire.
pub fn ext4_insert_extent_inline(
    i_block_data: &mut [u8; 60],
    leaf: Ext4ExtentLeaf,
) -> FsResult<()> {
    // SAFETY: mutable ref sur 60 octets, initialisés.
    let hdr_ptr = i_block_data.as_mut_ptr() as *mut Ext4ExtentHeader;
    let hdr = unsafe { hdr_ptr.read_unaligned() };
    if hdr.eh_magic != EXT4_EXT_MAGIC { return Err(FsError::Corrupt); }
    if hdr.eh_depth != 0 { return Err(FsError::NotSupported); } // doit être feuille
    if hdr.eh_entries >= hdr.eh_max { return Err(FsError::NoSpace); }

    let entries = hdr.eh_entries as usize;
    let base  = core::mem::size_of::<Ext4ExtentHeader>();
    let leaf_size = core::mem::size_of::<Ext4ExtentLeaf>();
    // Trouve la position d'insertion (trié par ee_block)
    let leaf_ptr_base = unsafe {
        i_block_data.as_mut_ptr().add(base) as *mut Ext4ExtentLeaf
    };
    let insert_pos = (0..entries).position(|i| {
        unsafe { (*leaf_ptr_base.add(i)).ee_block > leaf.ee_block }
    }).unwrap_or(entries);

    // Décale les feuilles après insert_pos
    for i in (insert_pos..entries).rev() {
        unsafe {
            let src  = leaf_ptr_base.add(i);
            let dst  = leaf_ptr_base.add(i + 1);
            dst.write_unaligned(src.read_unaligned());
        }
    }
    unsafe { leaf_ptr_base.add(insert_pos).write_unaligned(leaf); }

    // Met à jour le compteur
    let new_hdr = Ext4ExtentHeader { eh_entries: hdr.eh_entries + 1, ..hdr };
    unsafe { hdr_ptr.write_unaligned(new_hdr); }

    EXTENT_STATS.inserts.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// ExtentStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct ExtentStats {
    pub lookups:           AtomicU64,
    pub hits:              AtomicU64,
    pub misses:            AtomicU64,
    pub index_traversals:  AtomicU64,
    pub inserts:           AtomicU64,
    pub errors:            AtomicU64,
    /// Nombre de reflinks créés (cp --reflink).
    pub reflinks:          AtomicU64,
    /// Nombre de CoW effectués (écriture dans un bloc partagé).
    pub cow_performed:     AtomicU64,
    /// Incréments de refcount.
    pub refcount_incs:     AtomicU64,
    /// Décréments de refcount.
    pub refcount_decs:     AtomicU64,
}

impl ExtentStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self {
            lookups: z!(), hits: z!(), misses: z!(), index_traversals: z!(),
            inserts: z!(), errors: z!(), reflinks: z!(), cow_performed: z!(),
            refcount_incs: z!(), refcount_decs: z!(),
        }
    }
}

pub static EXTENT_STATS: ExtentStats = ExtentStats::new();
