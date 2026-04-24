// kernel/src/security/capability/namespace.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CAPABILITY NAMESPACE — Espace de nommage des capacités
// ═══════════════════════════════════════════════════════════════════════════════
//
// Chaque CapNamespace constitue un domaine d'ObjectId indépendant.
// Un ObjectId valide dans un namespace A n'est pas valide dans B.
//
// UTILISATION :
//   • kernel namespace : PID 0 (init) — capacités sur tous les objets noyau
//   • user namespace   : chaque conteneur/processus — vue restreinte
//   • driver namespace : Ring 1 — capabilities sur MMIO/IRQ/DMA uniquement
//
// PROPRIÉTÉ :
//   cross_namespace_verify() retourne TOUJOURS Err(ObjectNotFound) si les
//   namespaces source et cible diffèrent → isolation garantie.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::rights::Rights;
use super::table::CapTable;
use super::token::{CapObjectType, ObjectId};
use super::verify::CapError;

// ─────────────────────────────────────────────────────────────────────────────
// NamespaceId
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant unique d'un namespace de capacités.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct NamespaceId(pub u32);

impl NamespaceId {
    /// Namespace noyau — capabilities sur tous les objets système.
    pub const KERNEL: Self = Self(0);

    /// Namespace utilisateur par défaut.
    pub const USER_DEFAULT: Self = Self(1);

    /// Namespace pour les drivers Ring 1.
    pub const DRIVER: Self = Self(2);

    pub fn is_kernel(self) -> bool {
        self.0 == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Compteur global de namespaces
// ─────────────────────────────────────────────────────────────────────────────

static NS_COUNTER: AtomicU32 = AtomicU32::new(3); // 0=KERNEL, 1=USER_DEFAULT, 2=DRIVER réservés

/// Alloue un nouveau NamespaceId unique.
pub fn alloc_namespace_id() -> NamespaceId {
    let id = NS_COUNTER.fetch_add(1, Ordering::Relaxed);
    NamespaceId(id)
}

// ─────────────────────────────────────────────────────────────────────────────
// CapNamespace — domaine d'ObjectId
// ─────────────────────────────────────────────────────────────────────────────

/// Namespace de capacités — domain d'ObjectId indépendant.
///
/// Chaque namespace dispose de son propre compteur d'ObjectId et de sa propre CapTable.
/// Un ObjectId issu d'un namespace A ne peut jamais être vérifié dans un namespace B.
pub struct CapNamespace {
    /// Identifiant de ce namespace.
    id: NamespaceId,
    /// Table des capacités appartenant à ce namespace.
    table: CapTable,
    /// Compteur d'ObjectId local — incrémenté à chaque `alloc_object_id()`.
    id_counter: AtomicU64,
    /// Nombre d'utilisateurs de ce namespace (reference counting).
    refcount: AtomicU32,
}

// SAFETY: CapNamespace utilise uniquement des primitives atomiques + CapTable (Sync).
unsafe impl Send for CapNamespace {}
unsafe impl Sync for CapNamespace {}

impl CapNamespace {
    /// Crée un nouveau namespace vide.
    pub fn new(id: NamespaceId) -> Self {
        Self {
            id,
            table: CapTable::new(),
            id_counter: AtomicU64::new(1), // 0 réservé = INVALID
            refcount: AtomicU32::new(1),
        }
    }

    /// Retourne l'identifiant du namespace.
    #[inline(always)]
    pub fn id(&self) -> NamespaceId {
        self.id
    }

    /// Alloue un nouvel ObjectId unique dans ce namespace.
    ///
    /// Les ObjectId sont garantis uniques dans le namespace + encodent le NamespaceId
    /// dans les bits supérieurs.
    pub fn alloc_object_id(&self) -> ObjectId {
        let local = self.id_counter.fetch_add(1, Ordering::Relaxed);
        // Encode NamespaceId dans les 16 bits supérieurs de l'ObjectId
        let oid = ((self.id.0 as u64) << 48) | (local & 0x0000_FFFF_FFFF_FFFF);
        ObjectId::from_raw(oid)
    }

    /// Extrait le NamespaceId encodé dans un ObjectId.
    pub fn namespace_of(oid: ObjectId) -> NamespaceId {
        NamespaceId((oid.as_u64() >> 48) as u32)
    }

    /// Vérifie qu'un ObjectId appartient à ce namespace.
    pub fn owns(&self, oid: ObjectId) -> bool {
        Self::namespace_of(oid) == self.id
    }

    /// Accès à la CapTable interne.
    #[inline(always)]
    pub fn table(&self) -> &CapTable {
        &self.table
    }

    /// Accorde une capability dans ce namespace.
    pub fn grant(
        &self,
        rights: Rights,
        obj_type: CapObjectType,
    ) -> Result<(ObjectId, super::token::CapToken), CapError> {
        let oid = self.alloc_object_id();
        let token = self.table.grant(oid, rights, obj_type)?;
        Ok((oid, token))
    }

    /// Révoque toutes les capabilities sur un ObjectId.
    pub fn revoke(&self, oid: ObjectId) -> Result<(), CapError> {
        if !self.owns(oid) {
            return Err(CapError::ObjectNotFound);
        }
        super::revocation::revoke(&self.table, oid);
        Ok(())
    }

    /// Vérifie un token dans ce namespace.
    pub fn verify(&self, token: super::token::CapToken, required: Rights) -> Result<(), CapError> {
        // Rejet rapide si l'ObjectId n'appartient pas à ce namespace
        if !self.owns(token.object_id()) {
            return Err(CapError::ObjectNotFound);
        }
        super::verify::verify(&self.table, token, required)
    }

    /// Incrémente le refcount.
    pub fn acquire(&self) {
        self.refcount.fetch_add(1, Ordering::Relaxed);
    }

    /// Décrémente le refcount — retourne vrai si le namespace peut être détruit.
    pub fn release(&self) -> bool {
        self.refcount.fetch_sub(1, Ordering::Release) == 1
    }

    /// Retourne le refcount courant.
    pub fn refcount(&self) -> u32 {
        self.refcount.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Vérification cross-namespace — TOUJOURS Err
// ─────────────────────────────────────────────────────────────────────────────

/// Tente une vérification cross-namespace.
/// Retourne TOUJOURS Err(ObjectNotFound) si les namespaces diffèrent.
/// Propriété d'isolation garantie.
pub fn cross_namespace_verify(
    ns_a: &CapNamespace,
    ns_b: &CapNamespace,
    token: super::token::CapToken,
    required: Rights,
) -> Result<(), CapError> {
    if ns_a.id() != ns_b.id() {
        // Isolation : pas de vérification cross-namespace
        return Err(CapError::ObjectNotFound);
    }
    // Même namespace — délégation à verify standard
    ns_a.verify(token, required)
}
