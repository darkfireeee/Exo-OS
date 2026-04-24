// kernel/src/security/capability/rights.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// RIGHTS — Bitmask des droits d'accès (Exo-OS Security · Couche 2b)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ⚠️  PÉRIMÈTRE DE PREUVE FORMELLE — Toute modification ici IMPOSE une mise à
//     jour des preuves Coq/TLA+ dans /proofs/kernel_security/.
//
// PROPRIÉTÉS :
//   Rights est un bitmask u32 — opérations bitwise sûres.
//   RÈGLE CAP-03 : Les droits d'un token DÉLÉGUÉ ne peuvent jamais dépasser
//                  les droits de la source → vérification via `is_subset_of()`.
//
// CONVENTION :
//   Les bits 0..=5 sont les droits standards (READ, WRITE, EXEC, GRANT, REVOKE, DELEGATE).
//   Les bits 6..=15 sont des droits domaine-spécifiques (IPC, memoria, DMA…).
//   Les bits 16..=31 sont réservés aux extensions futures.
// ═══════════════════════════════════════════════════════════════════════════════

use core::fmt;
use core::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not, Sub};

// ─────────────────────────────────────────────────────────────────────────────
// Rights — type principal
// ─────────────────────────────────────────────────────────────────────────────

/// Bitmask des droits accordés par un CapToken.
///
/// # Invariant (prouvé Coq)
/// Tout `Rights` résultant d'une délégation est un sous-ensemble des droits
/// du délégant : `delegated.is_subset_of(source)` == true, toujours.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Rights(u32);

impl Rights {
    // ── Droits standards (bits 0..5) ─────────────────────────────────────────

    /// Aucun droit.
    pub const NONE: Self = Self(0);

    /// Lecture de l'objet référencé.
    pub const READ: Self = Self(1 << 0);

    /// Écriture dans l'objet référencé.
    pub const WRITE: Self = Self(1 << 1);

    /// Exécution (mémoire exécutable, lancement d'ELF, invocation IPC).
    pub const EXEC: Self = Self(1 << 2);

    /// Permission d'accorder ce droit à un tiers (subdélégation de GRANT).
    pub const GRANT: Self = Self(1 << 3);

    /// Permission de révoquer des tokens émis sur cet objet.
    pub const REVOKE: Self = Self(1 << 4);

    /// Permission de déléguer un sous-ensemble des droits possédés.
    pub const DELEGATE: Self = Self(1 << 5);

    // ── Droits domaines IPC (bits 6..9) ──────────────────────────────────────

    /// Connexion à un endpoint IPC.
    pub const IPC_CONNECT: Self = Self(1 << 6);

    /// Envoi de messages sur un endpoint.
    pub const IPC_SEND: Self = Self(1 << 7);

    /// Réception de messages sur un endpoint.
    pub const IPC_RECV: Self = Self(1 << 8);

    /// Contrôle du cycle de vie d'un endpoint (close, shutdown).
    pub const IPC_MANAGE: Self = Self(1 << 9);

    // ── Droits domaine Mémoire (bits 10..12) ─────────────────────────────────

    /// Mapper une VMA dans un espace d'adressage.
    pub const MEM_MAP: Self = Self(1 << 10);

    /// Modifier les permissions d'une VMA existante.
    pub const MEM_PROTECT: Self = Self(1 << 11);

    /// Libérer une VMA.
    pub const MEM_UNMAP: Self = Self(1 << 12);

    // ── Droits domaine Device/DMA (bits 13..15) ───────────────────────────────

    /// Accès MMIO à un périphérique.
    pub const DEV_MMIO: Self = Self(1 << 13);

    /// Allocation d'opérations DMA.
    pub const DEV_DMA: Self = Self(1 << 14);

    /// Contrôle IRQ (enable/disable sur ce device).
    pub const DEV_IRQ: Self = Self(1 << 15);

    // ── Champs composites utiles ─────────────────────────────────────────────

    /// Droits complets en lecture/écriture sans délégation.
    pub const READ_WRITE: Self = Self(Self::READ.0 | Self::WRITE.0);

    /// Droits de base IPC (connexion + envoi + réception).
    pub const IPC_BASIC: Self = Self(Self::IPC_CONNECT.0 | Self::IPC_SEND.0 | Self::IPC_RECV.0);

    /// Tous les droits sur un objet (ownership complet).
    pub const ALL: Self = Self(u32::MAX);

    // ─────────────────────────────────────────────────────────────────────────
    // Constructeurs
    // ─────────────────────────────────────────────────────────────────────────

    /// Crée depuis bits bruts — bits inconnus tronqués.
    #[inline(always)]
    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits)
    }

    /// Crée depuis bits bruts — retourne None si bits non reconnus.
    #[inline(always)]
    pub const fn from_bits(bits: u32) -> Option<Self> {
        // On accepte uniquement les bits 0..15
        if bits & !0x_0000_FFFF == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Requêtes
    // ─────────────────────────────────────────────────────────────────────────

    /// Lit la valeur binaire sous-jacente.
    #[inline(always)]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Vérifie qu'au MOINS les bits `required` sont présents.
    #[inline(always)]
    pub const fn contains(self, required: Rights) -> bool {
        (self.0 & required.0) == required.0
    }

    /// Vérifie que `self` est un sous-ensemble strict ou égal de `parent`.
    /// Utilisé pour valider la délégation — PROPRIÉTÉ PROUVÉE Coq.
    #[inline(always)]
    pub const fn is_subset_of(self, parent: Rights) -> bool {
        (self.0 & !parent.0) == 0
    }

    /// Retourne l'intersection des droits.
    #[inline(always)]
    pub const fn intersect(self, other: Rights) -> Rights {
        Self(self.0 & other.0)
    }

    /// Retourne l'union des droits.
    #[inline(always)]
    pub const fn union(self, other: Rights) -> Rights {
        Self(self.0 | other.0)
    }

    /// Retire les droits `remove` de `self`.
    #[inline(always)]
    pub const fn without(self, remove: Rights) -> Rights {
        Self(self.0 & !remove.0)
    }

    /// Vrai si aucun droit n'est présent.
    #[inline(always)]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Retourne un Rights sans aucun droit (sentinel pour « entrée absente »).
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Nombre de droits individuels activés (popcount).
    #[inline(always)]
    pub fn count(self) -> u32 {
        self.0.count_ones()
    }

    /// Retourne vrai si le droit DELEGATE est présent (nécessaire pour sous-déléguer).
    #[inline(always)]
    pub const fn can_delegate(self) -> bool {
        (self.0 & Self::DELEGATE.0) != 0
    }

    /// Retourne vrai si le droit REVOKE est présent.
    #[inline(always)]
    pub const fn can_revoke(self) -> bool {
        (self.0 & Self::REVOKE.0) != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Opérateurs bitwise
// ─────────────────────────────────────────────────────────────────────────────

impl BitOr for Rights {
    type Output = Self;
    #[inline(always)]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Rights {
    #[inline(always)]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for Rights {
    type Output = Self;
    #[inline(always)]
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for Rights {
    #[inline(always)]
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl Not for Rights {
    type Output = Self;
    #[inline(always)]
    fn not(self) -> Self {
        Self(!self.0)
    }
}

impl Sub for Rights {
    type Output = Self;
    #[inline(always)]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 & !rhs.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Affichage
// ─────────────────────────────────────────────────────────────────────────────

impl fmt::Debug for Rights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return write!(f, "Rights(NONE)");
        }
        write!(f, "Rights(")?;
        let mut first = true;
        let mut bit = |name: &str, mask: u32| -> fmt::Result {
            if self.0 & mask != 0 {
                if !first {
                    write!(f, "|")?;
                }
                write!(f, "{}", name)?;
                first = false;
            }
            Ok(())
        };
        bit("READ", 1 << 0)?;
        bit("WRITE", 1 << 1)?;
        bit("EXEC", 1 << 2)?;
        bit("GRANT", 1 << 3)?;
        bit("REVOKE", 1 << 4)?;
        bit("DELEGATE", 1 << 5)?;
        bit("IPC_CONNECT", 1 << 6)?;
        bit("IPC_SEND", 1 << 7)?;
        bit("IPC_RECV", 1 << 8)?;
        bit("IPC_MANAGE", 1 << 9)?;
        bit("MEM_MAP", 1 << 10)?;
        bit("MEM_PROTECT", 1 << 11)?;
        bit("MEM_UNMAP", 1 << 12)?;
        bit("DEV_MMIO", 1 << 13)?;
        bit("DEV_DMA", 1 << 14)?;
        bit("DEV_IRQ", 1 << 15)?;
        // Bits inconnus
        let unknown = self.0 & !0x0000_FFFF;
        if unknown != 0 {
            if !first {
                write!(f, "|")?;
            }
            write!(f, "UNKNOWN(0x{:08x})", unknown)?;
        }
        write!(f, ")")?;
        Ok(())
    }
}

impl fmt::Display for Rights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Conversions
// ─────────────────────────────────────────────────────────────────────────────

impl From<u32> for Rights {
    #[inline(always)]
    fn from(v: u32) -> Self {
        Self::from_bits_truncate(v)
    }
}

impl From<Rights> for u32 {
    #[inline(always)]
    fn from(r: Rights) -> u32 {
        r.0
    }
}

impl Default for Rights {
    fn default() -> Self {
        Self::NONE
    }
}
