//! captable.rs — Binding capability RÉEL pour ExoFS (TIER 0 sécurité v0.2.0).
//!
//! Remplace le faux `verify_cap` (bitmask auto-déclaré passé par l'appelant) par une
//! consultation de la **vraie** table de capabilities du process appelant
//! (`pcb.cap_table`, déjà dans le PCB, héritée au fork).
//!
//! Modèle : **object_id-keyed**. Une capability ExoFS = `(object_id → droits, génération)`
//! avec `type_tag = FileInode`. Les droits sont les **bits ExoFS bruts** stockés dans le
//! champ `rights` de la cap (la `CapTable` est agnostique aux bits ; `Rights::contains`
//! est un test de sous-ensemble bitwise correct). Mint à l'ouverture, vérif à chaque
//! opération, révocation par génération.
//!
//! RÈGLE SEC-CAP-01 : aucune opération ExoFS privilégiée ne doit s'autoriser sur un
//!                    bitmask fourni par l'appelant — toujours consulter `cap_table`.

use crate::fs::exofs::core::types::BlobId;
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::security::capability::{CapObjectType, ObjectId as CapObjectId, Rights};

use super::validation::{required_right_for, CapabilityType, EACCES, EPERM};

/// Objet « racine du système de fichiers » — cible des capabilities pour les
/// opérations globales (gc, quota, snapshot, import) qui ne portent pas sur un
/// objet précis. Accordée à init (PID1) au boot, déléguée aux services autorisés.
pub const FS_ROOT_OBJECT_ID: u64 = 0;

/// PID du process appelant (TCB courant via percpu). `None` hors contexte process
/// (appel kernel interne, ou test host où `gs` n'est pas un TCB → on ne lit PAS `gs`).
///
/// SÉMANTIQUE : en production les syscalls ExoFS sont TOUJOURS appelés depuis le
/// dispatch userspace (un process courant existe). `None` ne survient qu'en contexte
/// kernel/test = **privilégié** → les helpers `grant`/`check` traitent `None` comme
/// « autorisé/no-op » (cf. RÈGLE SEC-CAP-01, hypothèse H1 de l'audit).
#[cfg(test)]
#[inline]
fn caller_pid() -> Option<u32> {
    None
}

#[cfg(not(test))]
#[inline]
fn caller_pid() -> Option<u32> {
    // SAFETY: lecture du pointeur TCB courant publié par le scheduler dans le percpu.
    let tcb_raw = unsafe { crate::arch::x86_64::smp::percpu::read_current_tcb() };
    if tcb_raw == 0 {
        return None;
    }
    // SAFETY: tcb_raw != 0 ; pointe vers le TCB vivant du thread courant.
    let tcb =
        unsafe { &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock) };
    Some(tcb.pid.0)
}

#[inline]
fn cap_oid(object_id: u64) -> CapObjectId {
    CapObjectId::from_raw(object_id)
}

/// Convertit un BlobId ExoFS en clé u64 pour la cap_table.
///
/// L'`ObjectId` ExoFS est un hash blake3 de 32 octets (content-addressé). La `CapTable`
/// est indexée par `u64` → on dérive une clé stable = les 8 premiers octets du hash
/// (résistance aux collisions suffisante, le hash étant déjà cryptographique).
#[inline]
pub fn object_id_of_blob(blob_id: &BlobId) -> u64 {
    let oid = crate::fs::exofs::core::types::object_id_from_blob_id(blob_id);
    u64::from_le_bytes([
        oid.0[0], oid.0[1], oid.0[2], oid.0[3], oid.0[4], oid.0[5], oid.0[6], oid.0[7],
    ])
}

/// Accorde au process courant une capability ExoFS sur `object_id` avec `exofs_rights`
/// (bits ExoFS bruts). Idempotent : un re-grant met à jour les droits.
pub fn grant_object_cap(object_id: u64, exofs_rights: u32) -> Result<(), i64> {
    let pid = match caller_pid() {
        Some(p) => p,
        None => return Ok(()), // contexte kernel/test (privilégié) → no-op
    };
    let pcb = PROCESS_REGISTRY.find_by_pid(Pid(pid)).ok_or(EPERM)?;
    pcb.cap_table
        .grant(
            cap_oid(object_id),
            Rights::from_bits_truncate(exofs_rights),
            CapObjectType::FileInode,
        )
        .map(|_| ())
        .map_err(|_| EPERM)
}

/// Accorde une capability ExoFS à un PID **explicite** (bootstrap init, délégation).
pub fn grant_object_cap_to(pid: u32, object_id: u64, exofs_rights: u32) -> Result<(), i64> {
    let pcb = PROCESS_REGISTRY.find_by_pid(Pid(pid)).ok_or(EPERM)?;
    pcb.cap_table
        .grant(
            cap_oid(object_id),
            Rights::from_bits_truncate(exofs_rights),
            CapObjectType::FileInode,
        )
        .map(|_| ())
        .map_err(|_| EPERM)
}

/// Vérifie que le process courant détient une capability `FileInode` sur `object_id`
/// couvrant **au moins** les droits `required` (bits ExoFS). Retourne `EACCES` sinon.
pub fn check_object_cap(object_id: u64, required: u32) -> Result<(), i64> {
    let pid = match caller_pid() {
        Some(p) => p,
        None => return Ok(()), // contexte kernel/test (privilégié) → autorisé
    };
    let pcb = PROCESS_REGISTRY.find_by_pid(Pid(pid)).ok_or(EPERM)?;
    if pcb.cap_table.check_object(
        cap_oid(object_id),
        Rights::from_bits_truncate(required),
        CapObjectType::FileInode,
    ) {
        Ok(())
    } else {
        Err(EACCES)
    }
}

/// Variante par **fd** : dérive l'object_id depuis la table de fd ExoFS.
pub fn check_object_cap_fd(fd: u32, required: u32) -> Result<(), i64> {
    let blob_id = super::object_fd::OBJECT_TABLE
        .blob_id_of(fd)
        .map_err(|_| EPERM)?;
    check_object_cap(object_id_of_blob(&blob_id), required)
}

/// Variante par **blob_id** déjà résolu (chemins).
pub fn check_object_cap_blob(blob_id: &BlobId, required: u32) -> Result<(), i64> {
    check_object_cap(object_id_of_blob(blob_id), required)
}

/// Variante pour les opérations **globales** (gc/quota/snapshot/import) : exige une
/// capability sur l'objet racine `FS_ROOT_OBJECT_ID`.
pub fn check_fs_root_cap(required: u32) -> Result<(), i64> {
    check_object_cap(FS_ROOT_OBJECT_ID, required)
}

// ─────────────────────────────────────────────────────────────────────────────
// Wrappers par CapabilityType (churn minimal aux sites d'appel — remplacent verify_cap)
// ─────────────────────────────────────────────────────────────────────────────

/// Op portant sur un **fd** : vérifie la cap de l'objet du fd pour le droit requis.
#[inline]
pub fn check_fd(fd: u32, cap: CapabilityType) -> Result<(), i64> {
    check_object_cap_fd(fd, required_right_for(cap))
}

/// Op portant sur un **blob_id** résolu (chemin) : vérifie la cap pour le droit requis.
#[inline]
pub fn check_blob(blob_id: &BlobId, cap: CapabilityType) -> Result<(), i64> {
    check_object_cap_blob(blob_id, required_right_for(cap))
}

/// Op **globale** (gc/quota/snapshot/import) : exige la cap sur FS_ROOT.
#[inline]
pub fn check_root(cap: CapabilityType) -> Result<(), i64> {
    check_fs_root_cap(required_right_for(cap))
}

/// Libère la capability d'un objet (close/delete). Best-effort.
pub fn revoke_object_cap(object_id: u64) {
    if let Some(pid) = caller_pid() {
        if let Some(pcb) = PROCESS_REGISTRY.find_by_pid(Pid(pid)) {
            let _ = pcb.cap_table.remove(cap_oid(object_id));
        }
    }
}

/// Droits ExoFS accordés à l'ouverture, dérivés des flags POSIX d'open.
///
/// Politique TIER 0 = **permissive à l'open** (tout chemin ouvrable ⇒ on mint les droits
/// correspondant aux flags). Le durcissement « qui peut ouvrir quoi » est le TIER 1.
/// Le MÉCANISME (mint + vérif + révocation) est néanmoins réel dès maintenant.
pub fn rights_from_open_flags(flags: u32) -> u32 {
    use crate::fs::exofs::core::rights::{
        RIGHT_CREATE, RIGHT_DELETE, RIGHT_INSPECT_CONTENT, RIGHT_LIST, RIGHT_READ, RIGHT_SETMETA,
        RIGHT_STAT, RIGHT_WRITE,
    };
    use super::object_fd::open_flags;
    let mut r = RIGHT_STAT | RIGHT_LIST;
    if open_flags::can_read(flags) {
        r |= RIGHT_READ | RIGHT_INSPECT_CONTENT;
    }
    if open_flags::can_write(flags) {
        r |= RIGHT_WRITE | RIGHT_CREATE | RIGHT_DELETE | RIGHT_SETMETA;
    }
    r
}

// ─────────────────────────────────────────────────────────────────────────────
// TIER 0.6 — tests bout-en-bout du socle capability ExoFS
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tier06_e2e_tests {
    //! Preuve bout-en-bout du scénario exact demandé (PLAN-SECURITE-V020 §0.6) :
    //!   1. process SANS cap → opération **refusée** ;
    //!   2. cap accordée (mint à l'open, droits **dérivés des vrais flags**) → **autorisée**,
    //!      et bornée (RDONLY n'autorise pas WRITE, la cap ne couvre que l'objet visé) ;
    //!   3. révocation (close/delete) → de nouveau **refusée**.
    //!
    //! On exerce la VRAIE dérivation `rights_from_open_flags` et la VRAIE `CapTable`
    //! (== `pcb.cap_table` en production). Les helpers `grant_object_cap`/`check_object_cap`
    //! sont pid-gated (no-op en test host, cf. `caller_pid()`), on cible donc directement
    //! la table — exactement l'objet que ces helpers manipulent en production.
    use super::*;
    use crate::fs::exofs::core::rights::{
        RIGHT_CREATE, RIGHT_DELETE, RIGHT_INSPECT_CONTENT, RIGHT_LIST, RIGHT_READ, RIGHT_SETMETA,
        RIGHT_STAT, RIGHT_WRITE,
    };
    use crate::fs::exofs::syscall::object_fd::open_flags;
    use crate::security::capability::CapTable;

    #[inline]
    fn r(bits: u32) -> Rights {
        Rights::from_bits_truncate(bits)
    }

    /// La dérivation des droits depuis les flags d'open est correcte et **bornée**.
    #[test]
    fn open_flags_derive_exact_rights() {
        // RDONLY : lecture + métadonnées, AUCUN droit d'écriture.
        let ro = rights_from_open_flags(open_flags::O_RDONLY);
        assert_eq!(ro & RIGHT_READ, RIGHT_READ);
        assert_eq!(ro & RIGHT_INSPECT_CONTENT, RIGHT_INSPECT_CONTENT);
        assert_eq!(ro & (RIGHT_STAT | RIGHT_LIST), RIGHT_STAT | RIGHT_LIST);
        assert_eq!(ro & RIGHT_WRITE, 0, "RDONLY ne doit JAMAIS accorder WRITE");
        assert_eq!(ro & (RIGHT_CREATE | RIGHT_DELETE | RIGHT_SETMETA), 0);

        // RDWR : lecture + écriture complète.
        let rw = rights_from_open_flags(open_flags::O_RDWR);
        assert_eq!(rw & RIGHT_READ, RIGHT_READ);
        assert_eq!(rw & RIGHT_WRITE, RIGHT_WRITE);
        assert_eq!(
            rw & (RIGHT_CREATE | RIGHT_DELETE | RIGHT_SETMETA),
            RIGHT_CREATE | RIGHT_DELETE | RIGHT_SETMETA
        );

        // WRONLY : écriture sans lecture.
        let wo = rights_from_open_flags(open_flags::O_WRONLY);
        assert_eq!(wo & RIGHT_WRITE, RIGHT_WRITE);
        assert_eq!(wo & RIGHT_READ, 0, "WRONLY ne doit pas accorder READ");
    }

    /// E2E : **refusé sans cap → autorisé après mint (open) → refusé après révocation (close)**.
    #[test]
    fn e2e_deny_then_grant_then_revoke() {
        const OID: u64 = 0x000A_11CE; // object_id (clé dérivée du blake3 du chemin)
        let table = CapTable::new(); // == pcb.cap_table d'un process neuf

        // (1) SANS cap : toute opération est refusée.
        assert!(
            !table.check_object(cap_oid(OID), r(RIGHT_READ), CapObjectType::FileInode),
            "un process sans capability ne doit RIEN pouvoir lire"
        );

        // (2) MINT à l'open (RDONLY) : on accorde exactement les droits dérivés des flags.
        let granted = rights_from_open_flags(open_flags::O_RDONLY);
        table
            .grant(cap_oid(OID), r(granted), CapObjectType::FileInode)
            .expect("mint open cap");
        // READ autorisé...
        assert!(table.check_object(cap_oid(OID), r(RIGHT_READ), CapObjectType::FileInode));
        // ...mais WRITE refusé (cap bornée par les flags d'open RDONLY).
        assert!(
            !table.check_object(cap_oid(OID), r(RIGHT_WRITE), CapObjectType::FileInode),
            "une cap RDONLY ne doit JAMAIS autoriser l'écriture"
        );
        // ...et un objet voisin reste inaccessible (la cap ne couvre QUE OID).
        assert!(!table.check_object(cap_oid(OID + 1), r(RIGHT_READ), CapObjectType::FileInode));

        // (3) RÉVOCATION (close/delete) : la cap disparaît → de nouveau refusé.
        table.remove(cap_oid(OID)).expect("revoke on close");
        assert!(
            !table.check_object(cap_oid(OID), r(RIGHT_READ), CapObjectType::FileInode),
            "après close/révocation, l'accès doit être refusé"
        );
    }

    /// E2E : une cap RDWR autorise réellement l'écriture (la montée de droits est effective).
    #[test]
    fn e2e_rdwr_grants_write() {
        const OID: u64 = 0x0000_0B0B;
        let table = CapTable::new();
        let granted = rights_from_open_flags(open_flags::O_RDWR);
        table
            .grant(cap_oid(OID), r(granted), CapObjectType::FileInode)
            .unwrap();
        assert!(table.check_object(cap_oid(OID), r(RIGHT_WRITE), CapObjectType::FileInode));
        assert!(table.check_object(
            cap_oid(OID),
            r(RIGHT_READ | RIGHT_WRITE),
            CapObjectType::FileInode
        ));
    }
}
