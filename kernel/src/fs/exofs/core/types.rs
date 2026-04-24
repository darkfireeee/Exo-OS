// kernel/src/fs/exofs/core/types.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Types fondamentaux ExoFS — ObjectId, BlobId, EpochId, Extent, DiskOffset,
//   SnapshotId, TimeSpec, ByteRange, InlineData, PhysAddr
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLES :
//   • ONDISK-01 : structs on-disk → #[repr(C)] + types plain
//   • LOBJ-05   : comparaison ObjectId en temps constant ct_eq()
//   • HASH-01   : BlobId = Blake3(contenu brut NON-compressé)
//   • ARITH-01  : checked_add/checked_mul pour tout calcul d'offset disque
//   • ARITH-02  : align_up jamais sans vérifier overflow

use core::fmt;

// ─────────────────────────────────────────────────────────────────────────────
// ObjectId — identifiant stable d'un objet logique
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant unique d'un LogicalObject (L-Obj).
///
/// Classe 1 : Blake3(blob_id || owner_cap) — calculé UNE SEULE FOIS, immuable.
/// Classe 2 : compteur u64 monotone — stable à vie après création.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(C)]
pub struct ObjectId(pub [u8; 32]);

impl ObjectId {
    /// Identifiant invalide — ne correspond à aucun objet.
    pub const INVALID: Self = Self([0xFF; 32]);

    /// Comparaison en temps constant — résistance aux timing attacks (règle LOBJ-05).
    #[inline]
    pub fn ct_eq(&self, other: &Self) -> bool {
        let mut acc: u8 = 0;
        for i in 0..32 {
            acc |= self.0[i] ^ other.0[i];
        }
        acc == 0
    }

    /// Vrai si l'identifiant est manifestement invalide.
    #[inline]
    pub fn is_invalid(&self) -> bool {
        self.ct_eq(&Self::INVALID)
    }

    /// Retourne un slice sur les octets bruts.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Default for ObjectId {
    fn default() -> Self {
        Self::INVALID
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0[..8] {
            write!(f, "{:02x}", b)?;
        }
        write!(f, "…")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobId — identifiant content-addressed d'un blob physique
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant d'un PhysicalBlob (P-Blob).
///
/// RÈGLE HASH-01 : BlobId = Blake3(données AVANT compression et AVANT
/// chiffrement). Jamais calculé sur des données compressées.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(C)]
pub struct BlobId(pub [u8; 32]);

impl BlobId {
    pub const ZERO: Self = Self([0u8; 32]);

    #[inline]
    pub fn ct_eq(&self, other: &Self) -> bool {
        let mut acc: u8 = 0;
        for i in 0..32 {
            acc |= self.0[i] ^ other.0[i];
        }
        acc == 0
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Default for BlobId {
    fn default() -> Self {
        Self::ZERO
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochId — compteur monotone d'epoch de commit
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant d'un Epoch de commit.
///
/// Monotone croissant. La valeur 0 est invalide (jamais committée).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct EpochId(pub u64);

impl Default for EpochId {
    fn default() -> Self {
        Self(0)
    }
}

impl EpochId {
    pub const INVALID: Self = Self(0);

    #[inline]
    pub fn is_valid(self) -> bool {
        self.0 != 0
    }

    /// Retourne l'epoch suivante, sature à u64::MAX (wrap-check explicite).
    #[inline]
    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

impl fmt::Display for EpochId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Epoch({})", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SnapshotId
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant d'un snapshot (epoch permanent).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SnapshotId(pub u64);

// ─────────────────────────────────────────────────────────────────────────────
// DiskOffset — adresse absolue 64 bits sur le disque
// ─────────────────────────────────────────────────────────────────────────────

/// Offset absolu sur le périphérique bloc en octets.
///
/// RÈGLE ARITH-01 : utiliser checked_add() pour TOUT calcul d'offset.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct DiskOffset(pub u64);

impl DiskOffset {
    pub const INVALID: Self = Self(u64::MAX);
    pub const ZERO: Self = Self(0);
    pub fn zero() -> Self {
        Self(0)
    }

    /// Addition sûre : retourne None en cas d'overflow (règle ARITH-01).
    #[inline]
    pub fn add(self, len: u64) -> Option<Self> {
        self.0.checked_add(len).map(Self)
    }

    /// Secteur 512 octets correspondant à cet offset.
    #[inline]
    pub fn to_sector_512(self) -> u64 {
        self.0 / 512
    }

    /// Secteur 4096 octets correspondant à cet offset.
    #[inline]
    pub fn to_sector_4k(self) -> u64 {
        self.0 / 4096
    }

    /// Arrondit vers le haut au multiple de `align`.
    ///
    /// `align` doit être une puissance de 2 non nulle.
    /// Retourne None en cas d'overflow (règle ARITH-02).
    #[inline]
    pub fn align_up(self, align: u64) -> Option<Self> {
        debug_assert!(
            align > 0 && align.is_power_of_two(),
            "align_up: non power-of-2"
        );
        let mask = align - 1;
        self.0.checked_add(mask).map(|v| Self(v & !mask))
    }

    /// Arrondit vers le bas au multiple de `align`.
    ///
    /// `align` doit être une puissance de 2 non nulle.
    #[inline]
    pub fn align_down(self, align: u64) -> Self {
        debug_assert!(
            align > 0 && align.is_power_of_two(),
            "align_down: non power-of-2"
        );
        Self(self.0 & !(align - 1))
    }

    /// Vrai si l'offset est aligné sur `align` octets.
    #[inline]
    pub fn is_aligned(self, align: u64) -> bool {
        debug_assert!(align > 0 && align.is_power_of_two());
        self.0 & (align - 1) == 0
    }

    /// Distance signée en octets entre self et other (self - other).
    #[inline]
    pub fn distance(self, other: Self) -> Option<i64> {
        let a = self.0 as i128;
        let b = other.0 as i128;
        let d = a - b;
        if d < i64::MIN as i128 || d > i64::MAX as i128 {
            return None;
        }
        Some(d as i64)
    }

    /// Vrai si cet offset est l'offset invalide sentinelle.
    #[inline]
    pub fn is_invalid(self) -> bool {
        self == Self::INVALID
    }
}

impl fmt::Display for DiskOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DiskOff({:#x})", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Extent — plage contigüe de données sur disque
// ─────────────────────────────────────────────────────────────────────────────

/// Plage contigüe de données physiques {offset, len}.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Extent {
    /// Offset absolu du début de l'extent sur le disque.
    pub offset: DiskOffset,
    /// Longueur en octets.
    pub len: u64,
}

impl Extent {
    pub const EMPTY: Self = Self {
        offset: DiskOffset(0),
        len: 0,
    };

    /// Crée un extent depuis un offset et une longueur.
    #[inline]
    pub const fn new(offset: u64, len: u64) -> Self {
        Self {
            offset: DiskOffset(offset),
            len,
        }
    }

    /// Retourne l'offset de fin (exclusif). None en cas d'overflow.
    #[inline]
    pub fn end(self) -> Option<DiskOffset> {
        self.offset.add(self.len)
    }

    /// Vrai si l'extent est vide.
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Vrai si `off` est à l'intérieur de cet extent.
    #[inline]
    pub fn contains_offset(self, off: DiskOffset) -> bool {
        off.0 >= self.offset.0 && self.end().map(|e| off.0 < e.0).unwrap_or(false)
    }

    /// Vrai si les deux extents se chevauchent (intersection non vide).
    pub fn overlaps(self, other: Extent) -> bool {
        let a_end = match self.end() {
            Some(v) => v.0,
            None => return false,
        };
        let b_end = match other.end() {
            Some(v) => v.0,
            None => return false,
        };
        self.offset.0 < b_end && other.offset.0 < a_end
    }

    /// Calcule l'intersection de deux extents.
    ///
    /// Retourne None si l'intersection est vide ou en cas d'overflow.
    pub fn intersection(self, other: Extent) -> Option<Extent> {
        let start = self.offset.0.max(other.offset.0);
        let a_end = self.end()?.0;
        let b_end = other.end()?.0;
        let end = a_end.min(b_end);
        if start >= end {
            return None;
        }
        Some(Extent {
            offset: DiskOffset(start),
            len: end - start,
        })
    }

    /// Fusionne deux extents contigus ou adjacents en un seul.
    ///
    /// Retourne None si les extents ne sont pas contigus ou si overflow.
    pub fn merge(self, other: Extent) -> Option<Extent> {
        let a_end = self.end()?.0;
        let b_end = other.end()?.0;
        if a_end == other.offset.0 {
            // self est avant other
            Some(Extent {
                offset: self.offset,
                len: self.len.checked_add(other.len)?,
            })
        } else if b_end == self.offset.0 {
            // other est avant self
            Some(Extent {
                offset: other.offset,
                len: other.len.checked_add(self.len)?,
            })
        } else {
            None
        }
    }

    /// Divise l'extent à l'offset `split_at` (relatif au début de l'extent).
    ///
    /// Retourne (gauche, droite). Retourne None si `split_at` est hors limites.
    pub fn split(self, split_at: u64) -> Option<(Extent, Extent)> {
        if split_at == 0 || split_at >= self.len {
            return None;
        }
        let left = Extent {
            offset: self.offset,
            len: split_at,
        };
        let right = Extent {
            offset: DiskOffset(self.offset.0.checked_add(split_at)?),
            len: self.len - split_at,
        };
        Some((left, right))
    }

    /// Aligne cet extent vers le bas sur `block_size` pour l'offset,
    /// et vers le haut pour la fin (arrondi conservateur).
    pub fn expand_to_block_boundaries(self, block_size: u64) -> Option<Extent> {
        let aligned_start = self.offset.align_down(block_size);
        let raw_end = self.end()?.0;
        let aligned_end = DiskOffset(raw_end).align_up(block_size)?.0;
        let new_len = aligned_end.checked_sub(aligned_start.0)?;
        Some(Extent {
            offset: aligned_start,
            len: new_len,
        })
    }
}

impl fmt::Display for Extent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ext({:#x}+{:#x})", self.offset.0, self.len)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PhysAddr — adresse physique de page kernel
// ─────────────────────────────────────────────────────────────────────────────

/// Adresse physique RAM (pour DMA/allocation frames).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    /// Aligne vers le bas.
    #[inline]
    pub fn align_down(self, align: u64) -> Self {
        debug_assert!(align > 0 && align.is_power_of_two());
        Self(self.0 & !(align - 1))
    }

    /// Aligne vers le haut (None si overflow).
    #[inline]
    pub fn align_up(self, align: u64) -> Option<Self> {
        debug_assert!(align > 0 && align.is_power_of_two());
        self.0
            .checked_add(align - 1)
            .map(|v| Self(v & !(align - 1)))
    }
}

impl fmt::Display for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Phys({:#x})", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TimeSpec — horodatage kernel (ticks monotones, no_std)
// ─────────────────────────────────────────────────────────────────────────────

/// Horodatage basé sur les ticks du timer système.
///
/// Pas de liaison à une heure réelle (pas de timezone, pas de NTP).
/// Utilisé pour last_modified, last_accessed dans les métadonnées.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(C)]
pub struct TimeSpec {
    /// Secondes depuis le démarrage (ou epoch arbitraire).
    pub secs: u64,
    /// Nanosecondes [0, 999_999_999].
    pub nanos: u32,
    /// Padding pour alignement 16B.
    _pad: u32,
}

impl TimeSpec {
    /// Horodatage zéro (origine / valeur invalide).
    pub const ZERO: Self = Self {
        secs: 0,
        nanos: 0,
        _pad: 0,
    };

    /// Crée un TimeSpec depuis des secondes et nanosecondes.
    ///
    /// Normalise les nanos >= 1_000_000_000.
    pub fn new(secs: u64, nanos: u32) -> Self {
        let extra_secs = (nanos / 1_000_000_000) as u64;
        let norm_nanos = nanos % 1_000_000_000;
        Self {
            secs: secs.saturating_add(extra_secs),
            nanos: norm_nanos,
            _pad: 0,
        }
    }

    /// Crée depuis des millisecondes depuis l'origine.
    pub fn from_millis(ms: u64) -> Self {
        Self::new(ms / 1_000, ((ms % 1_000) * 1_000_000) as u32)
    }

    /// Durée en millisecondes (perte de précision en dessous de 1 ms).
    pub fn as_millis(self) -> u64 {
        self.secs
            .saturating_mul(1_000)
            .saturating_add((self.nanos / 1_000_000) as u64)
    }

    /// Soustraction saturante : retourne la durée entre deux timestamps.
    pub fn saturating_elapsed(self, older: TimeSpec) -> TimeSpec {
        if self <= older {
            return Self::ZERO;
        }
        let (mut s, mut n) = (self.secs - older.secs, self.nanos);
        if n < older.nanos {
            s = s.saturating_sub(1);
            n = n + 1_000_000_000 - older.nanos;
        } else {
            n -= older.nanos;
        }
        Self {
            secs: s,
            nanos: n,
            _pad: 0,
        }
    }

    /// Vrai si cet horodatage est plus récent que `other`.
    #[inline]
    pub fn is_after(self, other: TimeSpec) -> bool {
        self > other
    }
}

impl fmt::Display for TimeSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:09}s", self.secs, self.nanos)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ByteRange — plage logique d'octets dans un objet (vue utilisateur)
// ─────────────────────────────────────────────────────────────────────────────

/// Plage logique d'octets {start, len} dans un objet (pas sur disque).
///
/// Contrairement à `Extent`, ne porte pas d'adresse physique.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub len: u64,
}

impl ByteRange {
    pub const EMPTY: Self = Self { start: 0, len: 0 };

    /// Crée une ByteRange.
    #[inline]
    pub const fn new(start: u64, len: u64) -> Self {
        Self { start, len }
    }

    /// Retourne la position de fin (exclusive). None si overflow.
    #[inline]
    pub fn end(self) -> Option<u64> {
        self.start.checked_add(self.len)
    }

    /// Vrai si la plage est vide.
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Vrai si `offset` est dans la plage.
    #[inline]
    pub fn contains(self, offset: u64) -> bool {
        offset >= self.start && self.end().map(|e| offset < e).unwrap_or(false)
    }

    /// Intersection de deux ByteRanges.
    pub fn intersection(self, other: ByteRange) -> Option<ByteRange> {
        let start = self.start.max(other.start);
        let end = self.end()?.min(other.end()?);
        if start >= end {
            return None;
        }
        Some(ByteRange {
            start,
            len: end - start,
        })
    }

    /// Vrai si les deux plages se chevauchent.
    pub fn overlaps(self, other: ByteRange) -> bool {
        self.intersection(other).is_some()
    }

    /// Divise la plage à l'offset relatif `split_at`.
    pub fn split(self, split_at: u64) -> Option<(ByteRange, ByteRange)> {
        if split_at == 0 || split_at >= self.len {
            return None;
        }
        let left = ByteRange {
            start: self.start,
            len: split_at,
        };
        let right = ByteRange {
            start: self.start.checked_add(split_at)?,
            len: self.len - split_at,
        };
        Some((left, right))
    }
}

impl fmt::Display for ByteRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ByteRange({:#x}..{:#x})",
            self.start,
            self.end().unwrap_or(u64::MAX)
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// InlineData — données inline (petits objets ≤ INLINE_DATA_MAX_SIZE)
// ─────────────────────────────────────────────────────────────────────────────

/// Tampon de données inline pour petits objets (<= 128 octets).
///
/// Évite l'allocation d'un extent disque pour les petits blobs (configs, clés).
/// Taille maximale définie par `INLINE_DATA_MAX_SIZE` dans constants.rs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineData {
    buf: [u8; 128],
    len: usize,
}

impl InlineData {
    /// Buffer vide.
    pub const EMPTY: Self = Self {
        buf: [0u8; 128],
        len: 0,
    };

    /// Crée depuis un slice. Retourne None si len > 128.
    pub fn from_slice(data: &[u8]) -> Option<Self> {
        if data.len() > 128 {
            return None;
        }
        let mut s = Self::EMPTY;
        s.buf[..data.len()].copy_from_slice(data);
        s.len = data.len();
        Some(s)
    }

    /// Retourne les données.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Longueur des données.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Vrai si vide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for InlineData {
    fn default() -> Self {
        Self::EMPTY
    }
}
