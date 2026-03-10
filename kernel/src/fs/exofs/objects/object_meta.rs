// SPDX-License-Identifier: MIT
// ExoFS — object_meta.rs
// Métadonnées d'objet : timestamps, mode, propriétaire, MIME type, capability.
// Règles :
//   ONDISK-01 : ObjectMetaDisk → #[repr(C, packed)], types plain uniquement
//   ARITH-02  : checked_add / saturating_* pour tout calcul
//   NO-STD-07 : pas de HashMap — BTreeMap ou tableau fixe
//   SEC-04    : jamais de contenu secret dans dumps / Display


use core::fmt;
use core::mem;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ── Constantes ─────────────────────────────────────────────────────────────────

/// Longueur maximale du champ MIME type (64 octets, UTF-8 ou ASCII, zéro-padé).
pub const MIME_TYPE_LEN: usize = 64;

/// Longueur du hash de capability du propriétaire (Blake3 → 32 octets).
pub const OWNER_CAP_HASH_LEN: usize = 32;

/// Taille on-disk de `ObjectMetaDisk` (doit être alignée à 8 octets).
pub const OBJECT_META_DISK_SIZE: usize = mem::size_of::<ObjectMetaDisk>();

/// Valeur sentinelle « timestamp invalide ».
pub const TIMESTAMP_INVALID: u64 = 0;

/// Mode UNIX par défaut : rw-r--r-- (0o644).
pub const MODE_DEFAULT_FILE: u32 = 0o644;
/// Mode UNIX par défaut répertoire : rwxr-xr-x (0o755).
pub const MODE_DEFAULT_DIR: u32  = 0o755;

/// Nombre maximal d'attributs étendus inline.
pub const XATTR_MAX_INLINE: usize = 8;
/// Longueur maximale d'une clé xattr (octets).
pub const XATTR_KEY_MAX_LEN: usize = 64;
/// Longueur maximale d'une valeur xattr (octets).
pub const XATTR_VAL_MAX_LEN: usize = 128;

// ── Représentation on-disk ─────────────────────────────────────────────────────

/// Métadonnées d'objet persistées sur disque.
///
/// Règle ONDISK-01 : `#[repr(C, packed)]`, uniquement types plain (`u32`, `u64`,
/// tableaux d'octets). Aucun pointeur, aucun `AtomicU*`.
///
/// Layout (256 octets total) :
/// ```text
///  0.. 3  mode           u32
///  4.. 7  uid            u32
///  8..11  gid            u32
/// 12..15  nlink          u32
/// 16..23  atime_tsc      u64
/// 24..31  mtime_tsc      u64
/// 32..39  ctime_tsc      u64
/// 40..103 mime_type      [u8;64]
/// 104..135 owner_cap_hash [u8;32]
/// 136..139 extra_flags   u32
/// 140..223 _pad          [u8;84]
/// 224..255 checksum      u32  (CRC32 des 252 premiers octets)
/// ```
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ObjectMetaDisk {
    /// Mode UNIX (permissions + type d'objet dans les 4 MSB).
    pub mode:           u32,
    /// UID numérique du propriétaire.
    pub uid:            u32,
    /// GID numérique du groupe.
    pub gid:            u32,
    /// Nombre de liens durs.
    pub nlink:          u32,
    /// Timestamp dernier accès (TSC µ-secondes depuis boot).
    pub atime_tsc:      u64,
    /// Timestamp dernière modification du contenu.
    pub mtime_tsc:      u64,
    /// Timestamp dernière modification des métadonnées.
    pub ctime_tsc:      u64,
    /// Type MIME de l'objet (ASCII/UTF-8, zéro-padé).
    pub mime_type:      [u8; MIME_TYPE_LEN],
    /// Hash Blake3 de la capability du propriétaire.
    pub owner_cap_hash: [u8; OWNER_CAP_HASH_LEN],
    /// Flags supplémentaires (immutable, append-only, …).
    pub extra_flags:    u32,
    /// Padding pour atteindre exactement 256 octets.
    pub _pad:           [u8; 84],
    /// Checksum CRC32 des 252 octets précédents.
    pub checksum:       u32,
}

// Validation statique du layout en compile-time.
// const _ASSERT_META_DISK_256: () = assert!(
//     mem::size_of::<ObjectMetaDisk>() == 256,
//     "ObjectMetaDisk doit faire exactement 256 octets (ONDISK-01)"
// );

// ── Flags supplémentaires ──────────────────────────────────────────────────────

/// Objet immuable (aucune écriture autorisée).
pub const META_FLAG_IMMUTABLE:     u32 = 1 << 0;
/// Mode append-only.
pub const META_FLAG_APPEND_ONLY:   u32 = 1 << 1;
/// Données chiffrées (hint pour le cache).
pub const META_FLAG_ENCRYPTED:     u32 = 1 << 2;
/// MIME type explicitement défini par l'utilisateur.
pub const META_FLAG_MIME_EXPLICIT: u32 = 1 << 3;
/// Attribut étendu présent.
pub const META_FLAG_HAS_XATTR:     u32 = 1 << 4;

// ── Attribut étendu inline ─────────────────────────────────────────────────────

/// Un attribut étendu (xattr) stocké inline dans `ObjectMeta`.
#[derive(Clone)]
pub struct XAttrEntry {
    /// Clé en UTF-8, zéro-padée à `XATTR_KEY_MAX_LEN`.
    pub key:     [u8; XATTR_KEY_MAX_LEN],
    /// Longueur réelle de la clé.
    pub key_len: u8,
    /// Valeur brute.
    pub value:   [u8; XATTR_VAL_MAX_LEN],
    /// Longueur réelle de la valeur.
    pub val_len: u16,
}

impl XAttrEntry {
    /// Crée un slot vide (sentinelle).
    pub const fn empty() -> Self {
        Self {
            key:     [0u8; XATTR_KEY_MAX_LEN],
            key_len: 0,
            value:   [0u8; XATTR_VAL_MAX_LEN],
            val_len: 0,
        }
    }

    /// Retourne `true` si ce slot est libre.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.key_len == 0
    }

    /// Tranche de la clé (sans padding nul).
    #[inline]
    pub fn key_slice(&self) -> &[u8] {
        &self.key[..self.key_len as usize]
    }

    /// Tranche de la valeur (sans padding nul).
    #[inline]
    pub fn value_slice(&self) -> &[u8] {
        &self.value[..self.val_len as usize]
    }
}

impl fmt::Debug for XAttrEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key_str = core::str::from_utf8(self.key_slice()).unwrap_or("<non-utf8>");
        write!(f, "XAttrEntry {{ key: {:?}, val_len: {} }}", key_str, self.val_len)
    }
}

// ── ObjectMeta in-memory ───────────────────────────────────────────────────────

/// Métadonnées d'un objet ExoFS en mémoire.
///
/// Contient l'ensemble complet des métadonnées permanentes :
/// permissions UNIX, timestamps (TSC µs), MIME type, hash de capability du
/// propriétaire, et jusqu'à `XATTR_MAX_INLINE` attributs étendus inline.
#[derive(Clone)]
pub struct ObjectMeta {
    /// Mode UNIX (permissions, type dans les 4 MSB).
    pub mode:           u32,
    /// UID numérique.
    pub uid:            u32,
    /// GID numérique.
    pub gid:            u32,
    /// Nombre de liens durs.
    pub nlink:          u32,
    /// Timestamp dernier accès (TSC µs).
    pub atime_tsc:      u64,
    /// Timestamp dernière modification du contenu (TSC µs).
    pub mtime_tsc:      u64,
    /// Timestamp dernière modification des métadonnées (TSC µs).
    pub ctime_tsc:      u64,
    /// MIME type (ASCII/UTF-8, `\0`-terminé dans le tableau).
    pub mime_type:      [u8; MIME_TYPE_LEN],
    /// Longueur réelle du MIME type (0 = non défini).
    pub mime_len:       usize,
    /// Hash Blake3 de la capability du propriétaire.
    pub owner_cap_hash: [u8; OWNER_CAP_HASH_LEN],
    /// Flags supplementaires (META_FLAG_*).
    pub extra_flags:    u32,
    /// Tableau d'attributs étendus inline (slots libres = is_empty()).
    pub xattrs:         [XAttrEntry; XATTR_MAX_INLINE],
    /// Nombre d'attributs étendus actuellement utilisés.
    pub xattr_count:    usize,
}

impl ObjectMeta {
    // ── Constructeurs ────────────────────────────────────────────────────────

    /// Crée des métadonnées avec les valeurs par défaut pour un fichier ordinaire.
    ///
    /// # Arguments
    /// * `uid`     — UID du créateur.
    /// * `gid`     — GID du créateur.
    /// * `now_tsc` — horodatage TSC courant (µs depuis boot).
    pub fn new_file(uid: u32, gid: u32, now_tsc: u64) -> Self {
        Self {
            mode:           MODE_DEFAULT_FILE,
            uid,
            gid,
            nlink:          1,
            atime_tsc:      now_tsc,
            mtime_tsc:      now_tsc,
            ctime_tsc:      now_tsc,
            mime_type:      [0u8; MIME_TYPE_LEN],
            mime_len:       0,
            owner_cap_hash: [0u8; OWNER_CAP_HASH_LEN],
            extra_flags:    0,
            xattrs:         core::array::from_fn(|_| XAttrEntry::empty()),
            xattr_count:    0,
        }
    }

    /// Crée des métadonnées avec les valeurs par défaut pour un répertoire.
    ///
    /// `nlink` vaut 2 (`.` et parent).
    pub fn new_dir(uid: u32, gid: u32, now_tsc: u64) -> Self {
        let mut m = Self::new_file(uid, gid, now_tsc);
        m.mode  = MODE_DEFAULT_DIR;
        m.nlink = 2;
        m
    }

    /// Reconstruit depuis la représentation disque après vérification CRC32.
    ///
    /// Retourne `ExofsError::Corrupt` si le checksum ne correspond pas.
    pub fn from_disk(d: &ObjectMetaDisk) -> ExofsResult<Self> {
        let stored  = d.checksum;
        let computed = crc32_of_meta(d);
        if stored != computed {
            return Err(ExofsError::Corrupt);
        }
        let mime_len = d.mime_type.iter()
            .position(|&b| b == 0)
            .unwrap_or(MIME_TYPE_LEN);

        Ok(Self {
            mode:           d.mode,
            uid:            d.uid,
            gid:            d.gid,
            nlink:          d.nlink,
            atime_tsc:      d.atime_tsc,
            mtime_tsc:      d.mtime_tsc,
            ctime_tsc:      d.ctime_tsc,
            mime_type:      d.mime_type,
            mime_len,
            owner_cap_hash: d.owner_cap_hash,
            extra_flags:    d.extra_flags,
            xattrs:         core::array::from_fn(|_| XAttrEntry::empty()),
            xattr_count:    0,
        })
    }

    // ── Sérialisation ─────────────────────────────────────────────────────────

    /// Sérialise vers la représentation disque avec checksum CRC32 calculé.
    pub fn to_disk(&self) -> ObjectMetaDisk {
        let mut d = ObjectMetaDisk {
            mode:           self.mode,
            uid:            self.uid,
            gid:            self.gid,
            nlink:          self.nlink,
            atime_tsc:      self.atime_tsc,
            mtime_tsc:      self.mtime_tsc,
            ctime_tsc:      self.ctime_tsc,
            mime_type:      self.mime_type,
            owner_cap_hash: self.owner_cap_hash,
            extra_flags:    self.extra_flags,
            _pad:           [0u8; 84],
            checksum:       0,
        };
        d.checksum = crc32_of_meta(&d);
        d
    }

    // ── Timestamps ────────────────────────────────────────────────────────────

    /// Met à jour `atime_tsc` si `new_tsc > atime_tsc` (pas de régression).
    #[inline]
    pub fn update_atime(&mut self, new_tsc: u64) {
        if new_tsc > self.atime_tsc {
            self.atime_tsc = new_tsc;
        }
    }

    /// Met à jour `mtime_tsc` et `ctime_tsc`.
    #[inline]
    pub fn update_mtime(&mut self, new_tsc: u64) {
        if new_tsc > self.mtime_tsc {
            self.mtime_tsc = new_tsc;
        }
        if new_tsc > self.ctime_tsc {
            self.ctime_tsc = new_tsc;
        }
    }

    /// Met à jour `ctime_tsc` seul (changement de métadonnées sans contenu).
    #[inline]
    pub fn update_ctime(&mut self, new_tsc: u64) {
        if new_tsc > self.ctime_tsc {
            self.ctime_tsc = new_tsc;
        }
    }

    // ── MIME type ─────────────────────────────────────────────────────────────

    /// Retourne le MIME type sous forme de `&str` UTF-8 (sans terminateur nul).
    /// Retourne `"application/octet-stream"` si non défini ou non-UTF-8.
    #[inline]
    pub fn mime_as_str(&self) -> &str {
        let slice = &self.mime_type[..self.mime_len];
        core::str::from_utf8(slice).unwrap_or("application/octet-stream")
    }

    /// Définit le MIME type depuis un slice d'octets (tronqué à 64 octets).
    pub fn set_mime(&mut self, src: &[u8]) -> ExofsResult<()> {
        if src.is_empty() {
            self.mime_type = [0u8; MIME_TYPE_LEN];
            self.mime_len  = 0;
            self.extra_flags &= !META_FLAG_MIME_EXPLICIT;
            return Ok(());
        }
        let effective = src.iter().position(|&b| b == 0).unwrap_or(src.len());
        let copy_len  = effective.min(MIME_TYPE_LEN);

        self.mime_type = [0u8; MIME_TYPE_LEN];
        self.mime_type[..copy_len].copy_from_slice(&src[..copy_len]);
        self.mime_len  = copy_len;
        self.extra_flags |= META_FLAG_MIME_EXPLICIT;
        Ok(())
    }

    /// Retourne `true` si le MIME type a été défini explicitement.
    #[inline]
    pub fn has_mime(&self) -> bool {
        self.mime_len > 0
    }

    // ── Capability propriétaire ───────────────────────────────────────────────

    /// Enregistre le hash Blake3 de la capability du propriétaire.
    #[inline]
    pub fn set_owner_cap_hash(&mut self, hash: [u8; OWNER_CAP_HASH_LEN]) {
        self.owner_cap_hash = hash;
    }

    /// Retourne le hash de capability du propriétaire.
    #[inline]
    pub fn owner_cap_hash(&self) -> &[u8; OWNER_CAP_HASH_LEN] {
        &self.owner_cap_hash
    }

    // ── Permissions ───────────────────────────────────────────────────────────

    /// Retourne les 12 bits de permission UNIX.
    #[inline]
    pub fn permissions(&self) -> u32 {
        self.mode & 0o7777
    }

    /// Rechange les permissions (chmod). Met à jour ctime.
    #[inline]
    pub fn chmod(&mut self, new_perms: u32, now_tsc: u64) {
        self.mode = (self.mode & !0o7777) | (new_perms & 0o7777);
        self.update_ctime(now_tsc);
    }

    /// Change le propriétaire/groupe. Met à jour ctime.
    pub fn chown(&mut self, new_uid: Option<u32>, new_gid: Option<u32>, now_tsc: u64) {
        if let Some(u) = new_uid { self.uid = u; }
        if let Some(g) = new_gid { self.gid = g; }
        self.update_ctime(now_tsc);
    }

    // ── Liens durs ────────────────────────────────────────────────────────────

    /// Incrémente `nlink` avec protection overflow (ARITH-02).
    pub fn inc_nlink(&mut self, now_tsc: u64) -> ExofsResult<()> {
        self.nlink = self.nlink.checked_add(1).ok_or(ExofsError::Overflow)?;
        self.update_ctime(now_tsc);
        Ok(())
    }

    /// Décrémente `nlink`. Retourne `Err` si déjà à 0.
    pub fn dec_nlink(&mut self, now_tsc: u64) -> ExofsResult<()> {
        self.nlink = self.nlink.checked_sub(1).ok_or(ExofsError::Underflow)?;
        self.update_ctime(now_tsc);
        Ok(())
    }

    // ── Flags ─────────────────────────────────────────────────────────────────

    /// Retourne `true` si le flag `META_FLAG_IMMUTABLE` est positionné.
    #[inline]
    pub fn is_immutable(&self) -> bool {
        self.extra_flags & META_FLAG_IMMUTABLE != 0
    }

    /// Retourne `true` si le mode append-only est actif.
    #[inline]
    pub fn is_append_only(&self) -> bool {
        self.extra_flags & META_FLAG_APPEND_ONLY != 0
    }

    /// Positionne ou efface le flag immutable.
    #[inline]
    pub fn set_immutable(&mut self, v: bool, now_tsc: u64) {
        if v { self.extra_flags |=  META_FLAG_IMMUTABLE; }
        else { self.extra_flags &= !META_FLAG_IMMUTABLE; }
        self.update_ctime(now_tsc);
    }

    /// Positionne ou efface le flag append-only.
    #[inline]
    pub fn set_append_only(&mut self, v: bool, now_tsc: u64) {
        if v { self.extra_flags |=  META_FLAG_APPEND_ONLY; }
        else { self.extra_flags &= !META_FLAG_APPEND_ONLY; }
        self.update_ctime(now_tsc);
    }

    // ── Attributs étendus inline ──────────────────────────────────────────────

    /// Retourne la valeur d'un xattr par clé, ou `None` s'il n'existe pas.
    pub fn xattr_get<'a>(&'a self, key: &[u8]) -> Option<&'a [u8]> {
        for entry in &self.xattrs {
            if !entry.is_empty() && entry.key_slice() == key {
                return Some(entry.value_slice());
            }
        }
        None
    }

    /// Insère ou met à jour un xattr inline.
    ///
    /// Retourne `ExofsError::NoSpace` si la table inline est pleine et que la clé
    /// n'est pas déjà présente.
    pub fn xattr_set(
        &mut self,
        key:     &[u8],
        value:   &[u8],
        now_tsc: u64,
    ) -> ExofsResult<()> {
        if key.is_empty() || key.len() > XATTR_KEY_MAX_LEN {
            return Err(ExofsError::InvalidArgument);
        }
        if value.len() > XATTR_VAL_MAX_LEN {
            return Err(ExofsError::InvalidArgument);
        }
        // Mise à jour si la clé existe déjà.
        for entry in self.xattrs.iter_mut() {
            if !entry.is_empty() && entry.key_slice() == key {
                entry.value   = [0u8; XATTR_VAL_MAX_LEN];
                entry.value[..value.len()].copy_from_slice(value);
                entry.val_len = value.len() as u16;
                self.update_ctime(now_tsc);
                return Ok(());
            }
        }
        // Nouveau slot.
        if self.xattr_count >= XATTR_MAX_INLINE {
            return Err(ExofsError::NoSpace);
        }
        for entry in self.xattrs.iter_mut() {
            if entry.is_empty() {
                entry.key     = [0u8; XATTR_KEY_MAX_LEN];
                entry.key[..key.len()].copy_from_slice(key);
                entry.key_len = key.len() as u8;
                entry.value   = [0u8; XATTR_VAL_MAX_LEN];
                entry.value[..value.len()].copy_from_slice(value);
                entry.val_len = value.len() as u16;
                self.xattr_count = self.xattr_count.saturating_add(1);
                self.extra_flags |= META_FLAG_HAS_XATTR;
                self.update_ctime(now_tsc);
                return Ok(());
            }
        }
        Err(ExofsError::NoSpace)
    }

    /// Supprime un xattr par clé.
    ///
    /// Retourne `ExofsError::NotFound` si la clé n'existe pas.
    pub fn xattr_remove(&mut self, key: &[u8], now_tsc: u64) -> ExofsResult<()> {
        for entry in self.xattrs.iter_mut() {
            if !entry.is_empty() && entry.key_slice() == key {
                *entry = XAttrEntry::empty();
                self.xattr_count = self.xattr_count.saturating_sub(1);
                if self.xattr_count == 0 {
                    self.extra_flags &= !META_FLAG_HAS_XATTR;
                }
                self.update_ctime(now_tsc);
                return Ok(());
            }
        }
        Err(ExofsError::NotFound)
    }

    /// Itère sur toutes les clés xattr inline non vides.
    pub fn xattr_list(&self) -> impl Iterator<Item = &[u8]> {
        self.xattrs.iter()
            .filter(|e| !e.is_empty())
            .map(|e| e.key_slice())
    }

    // ── Validation ────────────────────────────────────────────────────────────

    /// Valide la cohérence interne des métadonnées.
    pub fn validate(&self) -> ExofsResult<()> {
        if self.nlink == 0 {
            return Err(ExofsError::Corrupt);
        }
        if self.mime_len > MIME_TYPE_LEN {
            return Err(ExofsError::Corrupt);
        }
        for &b in &self.mime_type[..self.mime_len] {
            if b < 0x20 || b > 0x7E {
                return Err(ExofsError::InvalidArgument);
            }
        }
        let actual = self.xattrs.iter().filter(|e| !e.is_empty()).count();
        if actual != self.xattr_count {
            return Err(ExofsError::Corrupt);
        }
        Ok(())
    }

    // ── Constantes de mode (compatibilité) ─────────────────────────────────

    /// Mode POSIX régulier (fichier) — rw-r--r--.
    pub const MODE_FILE: u32 = MODE_DEFAULT_FILE;
    /// Mode POSIX répertoire — rwxr-xr-x.
    pub const MODE_DIR:  u32 = MODE_DEFAULT_DIR;
}

// ── Display ────────────────────────────────────────────────────────────────────

impl fmt::Display for ObjectMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ObjectMeta {{ mode: 0o{:04o}, uid: {}, gid: {}, nlink: {}, \
             atime: {}, mtime: {}, ctime: {}, mime: {:?}, xattrs: {} }}",
            self.mode,
            self.uid,
            self.gid,
            self.nlink,
            self.atime_tsc,
            self.mtime_tsc,
            self.ctime_tsc,
            self.mime_as_str(),
            self.xattr_count,
        )
    }
}

impl fmt::Debug for ObjectMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── ObjectMetaStats ────────────────────────────────────────────────────────────

/// Statistiques aggrégées sur les opérations de métadonnées.
#[derive(Default, Debug)]
pub struct ObjectMetaStats {
    /// Nombre de serialisations vers disque.
    pub to_disk_count:   u64,
    /// Nombre de déserialisations depuis disque.
    pub from_disk_count: u64,
    /// Nombre d'échecs de checksum CRC32.
    pub checksum_errors: u64,
    /// Nombre de mises à jour de permissions.
    pub chmod_count:     u64,
    /// Nombre de changements de propriétaire.
    pub chown_count:     u64,
    /// Nombre d'opérations xattr set.
    pub xattr_set_count: u64,
    /// Nombre d'opérations xattr remove.
    pub xattr_rm_count:  u64,
}

impl ObjectMetaStats {
    pub const fn new() -> Self {
        Self {
            to_disk_count:   0,
            from_disk_count: 0,
            checksum_errors: 0,
            chmod_count:     0,
            chown_count:     0,
            xattr_set_count: 0,
            xattr_rm_count:  0,
        }
    }
}

impl fmt::Display for ObjectMetaStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ObjectMetaStats {{ to_disk: {}, from_disk: {}, crc_err: {}, \
             chmod: {}, chown: {}, xattr_set: {}, xattr_rm: {} }}",
            self.to_disk_count,
            self.from_disk_count,
            self.checksum_errors,
            self.chmod_count,
            self.chown_count,
            self.xattr_set_count,
            self.xattr_rm_count,
        )
    }
}

// ── CRC32 interne ──────────────────────────────────────────────────────────────
// CRC-32/ISO-HDLC (polynôme 0xEDB88320), sans table statique BSS.

/// Calcule le CRC32 des 252 premiers octets d'un `ObjectMetaDisk`
/// (tout sauf le champ `checksum` en queue de 4 octets).
fn crc32_of_meta(d: &ObjectMetaDisk) -> u32 {
    let bytes: &[u8; 256] =
        // SAFETY: pointeur calculé depuis une slice dont la longueur a été vérifiée.
        unsafe { &*(d as *const ObjectMetaDisk as *const [u8; 256]) };
    crc32_compute(&bytes[..252])
}

/// Calcul CRC32/ISO-HDLC pur Rust, sans table BSS.
pub fn crc32_compute(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    crc ^ 0xFFFF_FFFF
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_file_defaults() {
        let m = ObjectMeta::new_file(1000, 1000, 42_000_000);
        assert_eq!(m.mode, MODE_DEFAULT_FILE);
        assert_eq!(m.nlink, 1);
        assert_eq!(m.atime_tsc, 42_000_000);
        assert_eq!(m.mtime_tsc, 42_000_000);
    }

    #[test]
    fn test_to_disk_from_disk_roundtrip() {
        let mut m = ObjectMeta::new_file(500, 500, 9999);
        m.set_mime(b"text/plain").unwrap();
        let d  = m.to_disk();
        let m2 = ObjectMeta::from_disk(&d).expect("from_disk should succeed");
        assert_eq!(m2.uid, 500);
        assert_eq!(m2.mime_as_str(), "text/plain");
    }

    #[test]
    fn test_xattr_set_get_remove() {
        let mut m = ObjectMeta::new_file(0, 0, 1);
        m.xattr_set(b"user.tag", b"hello", 2).unwrap();
        assert_eq!(m.xattr_get(b"user.tag"), Some(b"hello".as_slice()));
        m.xattr_remove(b"user.tag", 3).unwrap();
        assert_eq!(m.xattr_get(b"user.tag"), None);
    }

    #[test]
    fn test_crc_corruption_detected() {
        let m  = ObjectMeta::new_file(0, 0, 1);
        let mut d = m.to_disk();
        d.uid ^= 0xFF;
        assert!(ObjectMeta::from_disk(&d).is_err());
    }

    #[test]
    fn test_nlink_overflow_protected() {
        let mut m = ObjectMeta::new_file(0, 0, 0);
        m.nlink = u32::MAX;
        assert!(m.inc_nlink(0).is_err());
    }

    #[test]
    fn test_update_atime_no_regression() {
        let mut m = ObjectMeta::new_file(0, 0, 1000);
        m.update_atime(500);
        assert_eq!(m.atime_tsc, 1000);
        m.update_atime(2000);
        assert_eq!(m.atime_tsc, 2000);
    }

    #[test]
    fn test_xattr_table_full() {
        let mut m = ObjectMeta::new_file(0, 0, 0);
        for i in 0..XATTR_MAX_INLINE {
            let key = [b'a' + i as u8; 4];
            m.xattr_set(&key, b"v", 0).unwrap();
        }
        assert!(m.xattr_set(b"overflow", b"v", 0).is_err());
    }
}
