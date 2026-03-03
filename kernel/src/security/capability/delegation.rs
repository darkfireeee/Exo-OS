// kernel/src/security/capability/delegation.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// DELEGATION — Subdélégation de capacités (Exo-OS Security · Couche 2b)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ⚠️  PÉRIMÈTRE DE VÉRIFICATION FORMELLE — Toute modification ici IMPOSE une mise à
//     jour des invariants dans INVARIANTS.md + réexécution de proptest.
//
// RÈGLE CAP-03 (vérifiée proptest + INVARIANTS.md — LAC-02) :
//   delegated_rights.is_subset_of(source_rights) TOUJOURS
//   Il est INTERDIT d'accorder plus de droits que ce qu'on possède.
//
// MODÈLE :
//   Le délégant possède un CapToken source (doit avoir le droit DELEGATE).
//   Il transmet un sous-ensemble de ses droits à la table cible.
//   La table cible reçoit un nouveau CapToken référençant le même ObjectId.
//
// CAS PARTICULIER — Délégation GRANT :
//   Seul un token avec le droit GRANT peut permettre à la table cible
//   de regrantiquer à son tour. Sans GRANT, la chaîne s'arrête.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use super::token::{CapToken, ObjectId};
use super::rights::Rights;
use super::table::CapTable;
use super::verify::{CapError, verify};

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de délégation
// ─────────────────────────────────────────────────────────────────────────────

/// Délègue un sous-ensemble de droits d'un token source vers une table cible.
///
/// # Préconditions
/// * `source_token` doit être valide dans `source_table`
/// * `source_token` doit posséder le droit `Rights::DELEGATE`
/// * `delegated_rights` doit être un sous-ensemble des droits de `source_token`
///
/// # Retourne
/// Un nouveau CapToken valide dans `target_table` avec `delegated_rights`.
///
/// # RÈGLE CAP-03 (proptest + INVARIANTS.md — LAC-02)
/// delegated.is_subset_of(source.rights) — vérifiée ici OBLIGATOIREMENT.
pub fn delegate(
    source_table:    &CapTable,
    source_token:    CapToken,
    target_table:    &CapTable,
    delegated_rights: Rights,
) -> Result<CapToken, CapError> {
    // 1. Vérifier que le délégant a bien le droit DELEGATE
    verify(source_table, source_token, Rights::DELEGATE)?;

    // 2. RÈGLE CAP-03 : les droits délégués NE peuvent PAS dépasser les droits source
    //    delegated_rights doit être un sous-ensemble de source_token.rights
    //    (sans DELEGATE lui-même, sauf si explicitement inclus et posé par l'appelant)
    if !delegated_rights.is_subset_of(source_token.rights()) {
        return Err(CapError::InsufficientRights);
    }

    // 3. Si delegated_rights contient GRANT → vérifier que source a GRANT aussi
    if delegated_rights.contains(Rights::GRANT) && !source_token.has_rights(Rights::GRANT) {
        return Err(CapError::InsufficientRights);
    }

    // 4. Accorder le nouveau token dans la table cible
    target_table.grant(
        source_token.object_id(),
        delegated_rights,
        source_token.object_type(),
    )
}

/// Délègue TOUS les droits du token source vers une table cible (sauf REVOKE).
/// Raccourci pour le cas courant où l'on clone une capability.
pub fn delegate_all(
    source_table:  &CapTable,
    source_token:  CapToken,
    target_table:  &CapTable,
) -> Result<CapToken, CapError> {
    // Délègue tous les droits sauf REVOKE — principle of least privilege.
    let delegated = source_token.rights() - Rights::REVOKE;
    delegate(source_table, source_token, target_table, delegated)
}

/// Délègue un token en lecture seule — retire tous les droits d'écriture/contrôle.
pub fn delegate_read_only(
    source_table:  &CapTable,
    source_token:  CapToken,
    target_table:  &CapTable,
) -> Result<CapToken, CapError> {
    let read_only = Rights::READ & source_token.rights();
    if read_only.is_empty() {
        return Err(CapError::InsufficientRights);
    }
    delegate(source_table, source_token, target_table, read_only)
}

/// Vérifie si une délégation est possible (sans modifier la table cible).
/// Utile pour pre-validation avant une opération complexe.
pub fn can_delegate(
    source_table:    &CapTable,
    source_token:    CapToken,
    delegated_rights: Rights,
) -> bool {
    // Vérification rapide sans modifier de table
    if source_token.is_invalid() {
        return false;
    }
    if !source_token.has_rights(Rights::DELEGATE) {
        return false;
    }
    if !delegated_rights.is_subset_of(source_token.rights()) {
        return false;
    }
    // Vérification de la génération (token non révoqué)
    verify(source_table, source_token, Rights::DELEGATE).is_ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// DelegationChain — audit du chemin de délégation
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans une chaîne de délégation.
#[derive(Debug, Clone, Copy)]
pub struct DelegationEntry {
    /// ObjectId de l'objet partagé.
    pub object_id:   ObjectId,
    /// Droits accordés à cette étape.
    pub rights:      Rights,
    /// Profondeur dans la chaîne (0 = propriétaire original).
    pub depth:       u8,
}

/// Chaîne de délégation — trace pour le journal d'audit.
/// Capacité maximale fixe (pas d'allocation heap sur le chemin critique).
pub struct DelegationChain {
    entries: [Option<DelegationEntry>; 8],
    len:     usize,
}

impl DelegationChain {
    pub const MAX_DEPTH: usize = 8;

    pub fn new() -> Self {
        Self {
            entries: [None; 8],
            len:     0,
        }
    }

    /// Ajoute une étape dans la chaîne.
    /// Retourne Err si la profondeur maximale est dépassée.
    pub fn push(&mut self, entry: DelegationEntry) -> Result<(), CapError> {
        if self.len >= Self::MAX_DEPTH {
            return Err(CapError::DelegationDenied);
        }
        self.entries[self.len] = Some(entry);
        self.len += 1;
        Ok(())
    }

    /// Nombre d'étapes dans la chaîne.
    pub fn depth(&self) -> usize {
        self.len
    }

    /// Itère sur les entrées.
    pub fn iter(&self) -> impl Iterator<Item = &DelegationEntry> {
        self.entries[..self.len].iter().filter_map(|e| e.as_ref())
    }
}

impl Default for DelegationChain {
    fn default() -> Self {
        Self::new()
    }
}
