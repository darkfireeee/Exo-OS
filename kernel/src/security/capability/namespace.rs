// kernel/src/security/capability/namespace.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CAPABILITY NAMESPACE — Capability naming space
// ═══════════════════════════════════════════════════════════════════════════════
//
// Each CapNamespace constitutes an independent ObjectId domain.
// An ObjectId valid in namespace A is not valid in B.
//
// USAGE:
//   • kernel namespace: PID 0 (init) — capabilities on all kernel objects
//   • user namespace  : each container/process — restricted view
//   • driver namespace: Ring 1 — capabilities on MMIO/IRQ/DMA only
//
// PROPERTY:
//   cross_namespace_verify() ALWAYS returns Err(ObjectNotFound) if
//   source and target namespaces differ -> guaranteed isolation.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::rights::Rights;
use super::table::CapTable;
use super::token::{CapObjectType, ObjectId};
use super::verify::CapError;

// ─────────────────────────────────────────────────────────────────────────────
// NamespaceId
// ─────────────────────────────────────────────────────────────────────────────

/// Unique identifier of a capability namespace.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct NamespaceId(pub u32);

impl NamespaceId {
    /// Kernel namespace — capabilities on all system objects.
    pub const KERNEL: Self = Self(0);

    /// Default user namespace.
    pub const USER_DEFAULT: Self = Self(1);

    /// Namespace for Ring 1 drivers.
    pub const DRIVER: Self = Self(2);

    pub fn is_kernel(self) -> bool {
        self.0 == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global namespace counter
// ─────────────────────────────────────────────────────────────────────────────

static NS_COUNTER: AtomicU32 = AtomicU32::new(3); // 0=KERNEL, 1=USER_DEFAULT, 2=DRIVER reserved

/// Allocates a new unique NamespaceId.
pub fn alloc_namespace_id() -> NamespaceId {
    let id = NS_COUNTER.fetch_add(1, Ordering::Relaxed);
    NamespaceId(id)
}

// ─────────────────────────────────────────────────────────────────────────────
// CapNamespace — domaine d'ObjectId
// ─────────────────────────────────────────────────────────────────────────────

/// Capability namespace — independent ObjectId domain.
///
/// Each namespace has its own ObjectId counter and CapTable.
/// An ObjectId from namespace A can never be verified in namespace B.
pub struct CapNamespace {
    /// Identifier of this namespace.
    id: NamespaceId,
    /// Capability table belonging to this namespace.
    table: CapTable,
    /// Local ObjectId counter — incremented on each `alloc_object_id()`.
    id_counter: AtomicU64,
    /// Number of users of this namespace (reference counting).
    refcount: AtomicU32,
}

// SAFETY: CapNamespace uses only atomic primitives + CapTable (Sync).
unsafe impl Send for CapNamespace {}
unsafe impl Sync for CapNamespace {}

impl CapNamespace {
    /// Creates a new empty namespace.
    pub fn new(id: NamespaceId) -> Self {
        Self {
            id,
            table: CapTable::new(),
            id_counter: AtomicU64::new(1), // 0 reserved = INVALID
            refcount: AtomicU32::new(1),
        }
    }

    /// Returns the namespace identifier.
    #[inline(always)]
    pub fn id(&self) -> NamespaceId {
        self.id
    }

    /// Allocates a new unique ObjectId in this namespace.
    ///
    /// ObjectIds are guaranteed unique in the namespace + encode NamespaceId
    /// in the upper bits.
    pub fn alloc_object_id(&self) -> ObjectId {
        let local = self.id_counter.fetch_add(1, Ordering::Relaxed);
        // Encode NamespaceId in upper 16 bits of ObjectId
        let oid = ((self.id.0 as u64) << 48) | (local & 0x0000_FFFF_FFFF_FFFF);
        ObjectId::from_raw(oid)
    }

    /// Extracts NamespaceId encoded in an ObjectId.
    pub fn namespace_of(oid: ObjectId) -> NamespaceId {
        NamespaceId((oid.as_u64() >> 48) as u32)
    }

    /// Verifies that an ObjectId belongs to this namespace.
    pub fn owns(&self, oid: ObjectId) -> bool {
        Self::namespace_of(oid) == self.id
    }

    /// Access to internal CapTable.
    #[inline(always)]
    pub fn table(&self) -> &CapTable {
        &self.table
    }

    /// Grants a capability in this namespace.
    pub fn grant(
        &self,
        rights: Rights,
        obj_type: CapObjectType,
    ) -> Result<(ObjectId, super::token::CapToken), CapError> {
        let oid = self.alloc_object_id();
        let token = self.table.grant(oid, rights, obj_type)?;
        Ok((oid, token))
    }

    /// Revokes all capabilities on an ObjectId.
    pub fn revoke(&self, oid: ObjectId) -> Result<(), CapError> {
        if !self.owns(oid) {
            return Err(CapError::ObjectNotFound);
        }
        super::revocation::revoke(&self.table, oid);
        Ok(())
    }

    /// Verifies a token in this namespace.
    pub fn verify(&self, token: super::token::CapToken, required: Rights) -> Result<(), CapError> {
        // Fast reject if ObjectId does not belong to this namespace
        if !self.owns(token.object_id()) {
            return Err(CapError::ObjectNotFound);
        }
        super::verify::verify(&self.table, token, required)
    }

    /// Increments refcount.
    pub fn acquire(&self) {
        self.refcount.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrements refcount — returns true if namespace can be destroyed.
    pub fn release(&self) -> bool {
        self.refcount.fetch_sub(1, Ordering::Release) == 1
    }

    /// Returns current refcount.
    pub fn refcount(&self) -> u32 {
        self.refcount.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Cross-namespace verification — ALWAYS Err
// ─────────────────────────────────────────────────────────────────────────────

/// Attempts a cross-namespace verification.
/// ALWAYS returns Err(ObjectNotFound) if namespaces differ.
/// Guaranteed isolation property.
pub fn cross_namespace_verify(
    ns_a: &CapNamespace,
    ns_b: &CapNamespace,
    token: super::token::CapToken,
    required: Rights,
) -> Result<(), CapError> {
    if ns_a.id() != ns_b.id() {
        // Isolation: no cross-namespace verification
        return Err(CapError::ObjectNotFound);
    }
    // Same namespace — delegate to standard verify
    ns_a.verify(token, required)
}
