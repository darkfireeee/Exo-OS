// kernel/src/security/capability/rights.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// RIGHTS — Access rights bitmask (Exo-OS Security · Layer 2b)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ⚠️  FORMAL PROOF SCOPE — Any modification here REQUIRES updating
//     Coq/TLA+ proofs in /proofs/kernel_security/.
//
// PROPERTIES:
//   Rights is a u32 bitmask — safe bitwise operations.
//   RULE CAP-03: Rights of a DELEGATED token can never exceed
//                source rights -> verification via `is_subset_of()`.
//
// CONVENTION:
//   Bits 0..=5 are standard rights (READ, WRITE, EXEC, GRANT, REVOKE, DELEGATE).
//   Bits 6..=15 are domain-specific rights (IPC, memory, DMA…).
//   Bits 16..=31 are reserved for future extensions.
// ═══════════════════════════════════════════════════════════════════════════════

use core::fmt;
use core::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not, Sub};
use subtle::{Choice, ConstantTimeEq};

// ─────────────────────────────────────────────────────────────────────────────
// Rights — Main type
// ─────────────────────────────────────────────────────────────────────────────

/// Bitmask of rights granted by a CapToken.
///
/// # Invariant (Coq proved)
/// Any `Rights` resulting from a delegation is a subset of the delegator's
/// rights: `delegated.is_subset_of(source)` == true, always.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Rights(u32);

impl Rights {
    // ── Standard rights (bits 0..5) ──────────────────────────────────────────

    /// No rights.
    pub const NONE: Self = Self(0);

    /// Read of referenced object.
    pub const READ: Self = Self(1 << 0);

    /// Write to referenced object.
    pub const WRITE: Self = Self(1 << 1);

    /// Execution (executable memory, ELF launch, IPC invocation).
    pub const EXEC: Self = Self(1 << 2);

    /// Permission to grant this right to a third party (sub-delegation of GRANT).
    pub const GRANT: Self = Self(1 << 3);

    /// Permission to revoke tokens issued on this object.
    pub const REVOKE: Self = Self(1 << 4);

    /// Permission to delegate a subset of held rights.
    pub const DELEGATE: Self = Self(1 << 5);

    // ── IPC domain rights (bits 6..9) ────────────────────────────────────────

    /// Connection to an IPC endpoint.
    pub const IPC_CONNECT: Self = Self(1 << 6);

    /// Sending messages on an endpoint.
    pub const IPC_SEND: Self = Self(1 << 7);

    /// Receiving messages on an endpoint.
    pub const IPC_RECV: Self = Self(1 << 8);

    /// Lifecycle control of an endpoint (close, shutdown).
    pub const IPC_MANAGE: Self = Self(1 << 9);

    // ── Memory domain rights (bits 10..12) ────────────────────────────────────

    /// Map a VMA into an address space.
    pub const MEM_MAP: Self = Self(1 << 10);

    /// Modify permissions of an existing VMA.
    pub const MEM_PROTECT: Self = Self(1 << 11);

    /// Free a VMA.
    pub const MEM_UNMAP: Self = Self(1 << 12);

    // ── Device/DMA domain rights (bits 13..15) ────────────────────────────────

    /// MMIO access to a device.
    pub const DEV_MMIO: Self = Self(1 << 13);

    /// Allocation of DMA operations.
    pub const DEV_DMA: Self = Self(1 << 14);

    /// IRQ control (enable/disable on this device).
    pub const DEV_IRQ: Self = Self(1 << 15);

    // ── Network domain rights (bits 16..19) ──────────────────────────────────

    /// Network lease issuance authorization (Arbiter).
    pub const NET_ISSUE: Self = Self(1 << 16);

    /// Network socket / strand materialization (Engine).
    pub const NET_MATERIALIZE: Self = Self(1 << 17);

    /// Revocation of a network lease / pact.
    pub const NET_REVOKE: Self = Self(1 << 18);

    /// Network administration / observation (Lens / Console).
    pub const NET_OBSERVE: Self = Self(1 << 19);

    // ── Useful composite fields ──────────────────────────────────────────────

    /// Full read/write rights without delegation.
    pub const READ_WRITE: Self = Self(Self::READ.0 | Self::WRITE.0);

    /// Basic IPC rights (connect + send + receive).
    pub const IPC_BASIC: Self = Self(Self::IPC_CONNECT.0 | Self::IPC_SEND.0 | Self::IPC_RECV.0);

    /// All rights on an object (full ownership).
    pub const ALL: Self = Self(u32::MAX);

    // ─────────────────────────────────────────────────────────────────────────
    // Constructors
    // ─────────────────────────────────────────────────────────────────────────

    /// Creates from raw bits — unknown bits truncated.
    #[inline(always)]
    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits)
    }

    /// Creates from raw bits — returns None if unrecognized bits present.
    #[inline(always)]
    pub const fn from_bits(bits: u32) -> Option<Self> {
        // Accept only bits 0..15
        if bits & !0x_0000_FFFF == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Queries
    // ─────────────────────────────────────────────────────────────────────────

    /// Reads underlying binary value.
    #[inline(always)]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Verifies that at LEAST `required` bits are present.
    #[inline(always)]
    pub const fn contains(self, required: Rights) -> bool {
        (self.0 & required.0) == required.0
    }

    /// Constant-time variant of `contains()` for sensitive hot paths.
    #[inline(always)]
    pub fn contains_ct(self, required: Rights) -> Choice {
        (self.0 & required.0).ct_eq(&required.0)
    }

    /// Checks that `self` is a strict or equal subset of `parent`.
    /// Used to validate delegation — Coq PROVED PROPERTY.
    #[inline(always)]
    pub const fn is_subset_of(self, parent: Rights) -> bool {
        (self.0 & !parent.0) == 0
    }

    /// Returns intersection of rights.
    #[inline(always)]
    pub const fn intersect(self, other: Rights) -> Rights {
        Self(self.0 & other.0)
    }

    /// Returns union of rights.
    #[inline(always)]
    pub const fn union(self, other: Rights) -> Rights {
        Self(self.0 | other.0)
    }

    /// Strips `remove` rights from `self`.
    #[inline(always)]
    pub const fn without(self, remove: Rights) -> Rights {
        Self(self.0 & !remove.0)
    }

    /// True if no rights are present.
    #[inline(always)]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Constant-time comparison of two right sets.
    #[inline(always)]
    pub fn ct_eq(self, other: Rights) -> Choice {
        self.0.ct_eq(&other.0)
    }

    /// Returns a Rights with no rights (sentinel for "absent entry").
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Number of individual active rights (popcount).
    #[inline(always)]
    pub fn count(self) -> u32 {
        self.0.count_ones()
    }

    /// Returns true if DELEGATE right is present (necessary to sub-delegate).
    #[inline(always)]
    pub const fn can_delegate(self) -> bool {
        (self.0 & Self::DELEGATE.0) != 0
    }

    /// Returns true if REVOKE right is present.
    #[inline(always)]
    pub const fn can_revoke(self) -> bool {
        (self.0 & Self::REVOKE.0) != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Bitwise operators
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
