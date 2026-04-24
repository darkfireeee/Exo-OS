// libs/exo-types/src/fixed_string.rs
//
// Fichier : libs/exo_types/src/fixed_string.rs
// Rôle    : FixedString<N>, ServiceName, PathBuf — GI-01 Étape 6.
//
// INVARIANTS :
//   - IPC-02 : Taille fixe, Sized, Copy — utilisable dans protocol.rs Ring 1.
//   - CORR-30 : len stocké en u32 (pas usize) pour ABI stable cross-arch.
//   - FixedString<N> remplace tous les &str / String dans les messages IPC.
//
// SOURCE DE VÉRITÉ :
//   ExoOS_Arborescence_V3.md §2 (libs/exo-types/), GI-01_Types_TCB_SSR.md §6,
//   ExoOS_Corrections_01 CORR-30

/// Chaîne UTF-8 de longueur fixe — `no_std` safe, `Copy`, `Sized`.
///
/// **N** = capacité maximale en bytes.
///
/// **CORR-30** : `len` est un `u32` (pas `usize`) pour ABI stable.
/// La taille totale de la structure est `N + 4` bytes arrondis à l'alignement.
///
/// # Utilisation dans les protocoles IPC
/// ```ignore
/// pub struct OpenRequest {
///     pub path:  PathBuf,    // FixedString<512>
///     pub flags: u32,
/// }
/// ```
///
/// ❌ INTERDIT dans les `protocol.rs` Ring 1 :
///   `&str`, `String`, `Vec<u8>`, `Box<str>`
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FixedString<const N: usize> {
    /// Données UTF-8 — seuls `bytes[0..len]` sont valides.
    pub bytes: [u8; N],
    /// Longueur en bytes (u32 pour ABI stable — CORR-30).
    pub len: u32,
}

impl<const N: usize> FixedString<N> {
    /// Chaîne vide.
    pub const EMPTY: Self = Self {
        bytes: [0u8; N],
        len: 0,
    };

    /// Construit depuis un slice de bytes UTF-8.
    ///
    /// Tronque silencieusement si `data.len() > N`.
    pub fn from_bytes(data: &[u8]) -> Self {
        let copy_len = data.len().min(N);
        let mut s = Self::EMPTY;
        s.bytes[..copy_len].copy_from_slice(&data[..copy_len]);
        s.len = copy_len as u32;
        s
    }

    /// Construit depuis un literal `&str` (compile-time si possible).
    pub fn from_str(s: &str) -> Self {
        Self::from_bytes(s.as_bytes())
    }

    /// Retourne le slice de bytes valides.
    #[inline(always)]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Tente de convertir en `&str` UTF-8.
    pub fn as_str(&self) -> Result<&str, core::str::Utf8Error> {
        core::str::from_utf8(self.as_bytes())
    }

    /// Longueur en bytes.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Vrai si la chaîne est vide.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Capacité maximale.
    #[inline(always)]
    pub const fn capacity() -> usize {
        N
    }

    /// Égalité avec un slice de bytes.
    pub fn eq_bytes(&self, other: &[u8]) -> bool {
        self.as_bytes() == other
    }
}

impl<const N: usize> PartialEq for FixedString<N> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self.bytes[..self.len as usize] == other.bytes[..other.len as usize]
    }
}

impl<const N: usize> Eq for FixedString<N> {}

impl<const N: usize> Default for FixedString<N> {
    fn default() -> Self {
        Self::EMPTY
    }
}

// ─── Alias canoniques ─────────────────────────────────────────────────────────

/// Nom de service IPC — 64 bytes + 4B len = 68B total.
///
/// Utilisé dans `Register { name: ServiceName, cap: CapToken }`.
pub type ServiceName = FixedString<{ crate::constants::SERVICE_NAME_LEN }>;

/// Chemin de fichier VFS — 512 bytes + 4B len = 516B total.
///
/// Utilisé dans `Open { path: PathBuf, flags: u32 }`.
pub type PathBuf = FixedString<{ crate::constants::PATH_BUF_LEN }>;

// ─── Tests ────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s: FixedString<64> = FixedString::from_str("hello");
        assert_eq!(s.as_str().unwrap(), "hello");
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn truncation() {
        let s: FixedString<4> = FixedString::from_str("hello");
        assert_eq!(s.len(), 4);
        assert_eq!(s.as_bytes(), b"hell");
    }

    #[test]
    fn equality() {
        let a: FixedString<32> = FixedString::from_str("test");
        let b: FixedString<32> = FixedString::from_str("test");
        let c: FixedString<32> = FixedString::from_str("other");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn service_name_size() {
        // ServiceName doit être Sized et sans alloc
        let name = ServiceName::from_str("vfs_server");
        assert_eq!(name.as_str().unwrap(), "vfs_server");
    }

    #[test]
    fn path_buf_size() {
        let path = PathBuf::from_str("/proc/self/status");
        assert!(path.as_str().is_ok());
    }
}
