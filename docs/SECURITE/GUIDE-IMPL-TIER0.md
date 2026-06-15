# Guide d'implémentation TIER 0 — Binding capability ExoFS (prêt à appliquer)

> Objectif : appliquer mécaniquement au prochain budget. Tout le code et les points
> d'insertion sont ci-dessous. Modèle = **object_id-keyed cap_table** (cf PLAN §Design TIER 0).

## Primitives validées (ne pas re-vérifier)

- PCB : `pcb.cap_table: Box<CapTable>` ([pcb.rs:526](../../kernel/src/process/core/pcb.rs)), créée vide ([pcb.rs:604](../../kernel/src/process/core/pcb.rs)).
- Accès : `PROCESS_REGISTRY.find_by_pid(Pid) -> Option<&ProcessControlBlock>` ([registry.rs:196](../../kernel/src/process/core/registry.rs)) → `&pcb.cap_table`.
- `CapTable::grant(ObjectId, Rights, CapObjectType) -> Result<CapToken, CapError>` ; `get(ObjectId) -> Option<CapEntryView{rights,generation,type_tag}>` ; `remove(ObjectId)` ; `inherit_from(&CapTable)`.
- `Rights::from_bits_truncate(u32)` = wrap fidèle (préserve TOUS les bits) ; `Rights::contains(req)` = subset bitwise.
- `CapObjectType::FileInode = 3` (tag pour objets ExoFS).
- ExoFS : `object_id_from_blob_id(&BlobId) -> ObjectId` ; `OBJECT_TABLE.blob_id_of(fd) -> ExofsResult<BlobId>` ; `resolve_path_to_blob(path, flags)` ; `required_right_for(CapabilityType) -> u32` ([validation.rs:140-166](../../kernel/src/fs/exofs/syscall/validation.rs)).

## ⚠️ Gaps confirmés à corriger

1. **fork n'appelle PAS `inherit_from`** — l'enfant reçoit une CapTable vide. → câbler (0.2).
2. **`verify_cap` est faux** (bitmask auto-déclaré, 24 sites). → remplacer par `check_object_cap` (0.4).
3. **cap_table jamais peuplée** — aucun `grant()` sur chemin réel. → mint à l'open + grant boot init (0.3/0.2).

---

## Étape A — Helper cap ExoFS (NOUVEAU fichier `kernel/src/fs/exofs/syscall/captable.rs`)

```rust
//! Capability binding réel pour ExoFS (TIER 0). object_id-keyed sur pcb.cap_table.
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::security::capability::{CapObjectType, ObjectId as CapObjectId, Rights};
use super::validation::EPERM; // i64

/// PID du process appelant (TCB courant via percpu). Renvoie None hors contexte process.
fn caller_pid() -> Option<u32> {
    let tcb = unsafe { crate::arch::x86_64::smp::percpu::read_current_tcb() };
    if tcb == 0 { return None; }
    let tcb = unsafe { &*(tcb as *const crate::scheduler::core::task::ThreadControlBlock) };
    Some(tcb.pid.0)
}

#[inline] fn coid(object_id: u64) -> CapObjectId { CapObjectId(object_id) }

/// Mint : accorde au process courant une cap ExoFS (bits ExoFS bruts) sur `object_id`.
pub fn grant_object_cap(object_id: u64, exofs_rights: u32) -> Result<(), i64> {
    let pid = caller_pid().ok_or(EPERM)?;
    let pcb = PROCESS_REGISTRY.find_by_pid(Pid(pid)).ok_or(EPERM)?;
    pcb.cap_table
        .grant(coid(object_id), Rights::from_bits_truncate(exofs_rights), CapObjectType::FileInode)
        .map(|_| ())
        .map_err(|_| EPERM)
}

/// Vérifie que le process courant détient une cap FileInode pour `object_id` avec `required`.
pub fn check_object_cap(object_id: u64, required: u32) -> Result<(), i64> {
    let pid = caller_pid().ok_or(EPERM)?;
    let pcb = PROCESS_REGISTRY.find_by_pid(Pid(pid)).ok_or(EPERM)?;
    match pcb.cap_table.get(coid(object_id)) {
        Some(v) if v.type_tag == CapObjectType::FileInode
                && v.rights.contains(Rights::from_bits_truncate(required)) => Ok(()),
        _ => Err(EPERM),
    }
}

/// Libère la cap (close/delete).
pub fn revoke_object_cap(object_id: u64) {
    if let Some(pid) = caller_pid() {
        if let Some(pcb) = PROCESS_REGISTRY.find_by_pid(Pid(pid)) {
            let _ = pcb.cap_table.remove(coid(object_id));
        }
    }
}

/// Droits ExoFS accordés à l'ouverture selon les flags POSIX.
pub fn rights_from_open_flags(flags: u32) -> u32 {
    use crate::fs::exofs::core::rights::*;
    use crate::fs::exofs::syscall::object_fd::open_flags;
    let mut r = RIGHT_STAT | RIGHT_LIST;
    if open_flags::can_read(flags)  { r |= RIGHT_READ | RIGHT_INSPECT_CONTENT; }
    if open_flags::can_write(flags) { r |= RIGHT_WRITE | RIGHT_CREATE | RIGHT_DELETE | RIGHT_SETMETA; }
    r
}
```
- Ajouter `mod captable;` dans `fs/exofs/syscall/mod.rs` (+ `pub use` si besoin).
- Vérifier le chemin exact de `read_current_tcb` (sinon réutiliser un `current_pid()` existant : fs_posix.rs:19, fast_path.rs:136).

## Étape B — fork inherit (0.2) — **ONE-LINER, simple**

⚠️ Correction : fork.rs:669 était du **code de test** (`insert_test_pcb`). Le **vrai**
do_fork crée `let mut child_pcb = ProcessControlBlock::try_new(...)` à
[fork.rs:530](../../kernel/src/process/lifecycle/fork.rs), **mutable**, et clone déjà les
namespaces du parent (`child_pcb.pid_ns.clone_from(&parent_pcb.pid_ns)` … lignes 560-564),
puis `PROCESS_REGISTRY.insert(child_pcb)` à [fork.rs:588](../../kernel/src/process/lifecycle/fork.rs).

→ **Insérer à ~ligne 565** (juste après les `clone_from` de namespaces, AVANT l'insert), `cap_table` étant `pub` :
```rust
// FIX-SEC-T0.2 : l'enfant hérite la CapTable du parent (capabilities = fds/objets ouverts).
child_pcb.cap_table =
    alloc::boxed::Box::new(crate::security::capability::CapTable::inherit_from(&parent_pcb.cap_table));
```
`parent_pcb` est déjà en scope dans do_fork (utilisé pour `name_snapshot`, `clone_from`…).
Pas de nouveau constructeur, pas de problème de `&PCB` immuable (on est avant l'insert).
`inherit_from` copie tous les slots non-libres sous write_lock ([table.rs:189](../../kernel/src/security/capability/table.rs)). ✅

## Étape C — Mint à l'open (0.3) — [object_open.rs:235](../../kernel/src/fs/exofs/syscall/object_open.rs)

Remplacer le bloc `verify_cap(cap_rights, cap_type)` par : ouvrir d'abord (open_object), puis grant.
```rust
// (supprimer le verify_cap ici)
let fd = match open_object(&path_buf, actual_len, &open_args) { Ok(f)=>f, Err(e)=>return exofs_err_to_errno(e) };
// mint : object_id du blob ouvert
if let Ok(bid) = OBJECT_TABLE.blob_id_of(fd) {
    let oid = object_id_from_blob_id(&bid).0;
    if let Err(e) = captable::grant_object_cap(oid, captable::rights_from_open_flags(open_args.flags)) {
        OBJECT_TABLE.close(fd); return e;
    }
}
```
(garder l'écriture out_fd ensuite). `open_by_path.rs` : idem (mint au lieu de verify).

## Étape D — Routage des 24 sites (0.4)

Patron : `verify_cap(cap_rights, T)` → `check_object_cap(object_id, required_right_for(T))`.
`object_id` selon la source :

| Fichier:ligne | CapabilityType | Source object_id |
|---|---|---|
| object_read.rs:242 | ExoFsObjectRead | **fd** → `OBJECT_TABLE.blob_id_of(fd)` |
| object_write.rs:270 | ExoFsObjectWrite | **fd** |
| object_stat.rs:199 | ExoFsObjectStat | **fd** (branche USE_FD) |
| object_stat.rs:216 | ExoFsObjectStat | **path** (branche path) |
| object_delete.rs:172 | ExoFsObjectDelete | **path** → `resolve_path_to_blob` |
| object_create.rs:321 | ExoFsObjectCreate | **path** (puis mint la nouvelle cap) |
| object_set_meta.rs:365/410 | ExoFsObjectSetMeta | **path/fd** (selon args) |
| get_content_hash.rs:167 | ExoFsGetContentHash | **path/fd** |
| object_open.rs:235 | (mint, étape C) | — |
| open_by_path.rs:129 | (mint) | path |
| path_resolve.rs:281 | ExoFsPathResolve | **path** |
| readdir.rs:238 | ExoFsReaddir | **path** (dir) |
| relation_create.rs:326 | ExoFsRelationCreate | **path** (objet source) |
| relation_query.rs:235 | ExoFsRelationQuery | **path** |
| snapshot_create.rs:253 | ExoFsSnapshotCreate | **global** → FS_ROOT cap |
| snapshot_list.rs:267 | ExoFsSnapshotList | **global** |
| snapshot_mount.rs:212 | ExoFsSnapshotMount | **global** |
| export_object.rs:230 | ExoFsExportObject | **path** |
| import_object.rs:181 | ExoFsImportObject | **global/path** |
| gc_trigger.rs:290 | ExoFsGcTrigger | **global** → FS_ROOT cap |
| quota_query.rs:349 | (Query/Set) | **global** → FS_ROOT cap |
| epoch_commit.rs:628 | ExoFsEpochCommit | **path/global** |

**Ops « global »** : définir `const FS_ROOT_OBJECT_ID: u64 = 0;` (ou un magic dédié) ; `check_object_cap(FS_ROOT_OBJECT_ID, required)`. Au boot, accorder à **init (PID1)** une cap admin sur FS_ROOT (étape E).

## Étape E — Bootstrap de confiance (0.2)

Au handoff init / création PID1 : `grant` à init une cap `RIGHT_ADMIN|ALL` (bits ExoFS) sur `FS_ROOT_OBJECT_ID`, pour que les ops globales (gc/quota/snapshot) et l'admin marchent. **Point d'insertion exact** : [create.rs:291-301](../../kernel/src/process/lifecycle/create.rs) (chemin `Pid::INIT`, après création du PCB d'init et son enregistrement dans `PROCESS_REGISTRY`). Faire `init_pcb.cap_table.grant(coid(FS_ROOT_OBJECT_ID), Rights::from_bits_truncate(ALL_RIGHTS|RIGHT_ADMIN), FileInode)`. Les serveurs reçoivent leurs caps par délégation depuis init (TIER 1) ou héritent au fork.
(`read_current_tcb` confirmé : [percpu.rs:279](../../kernel/src/arch/x86_64/smp/percpu.rs).)

## Hypothèses & risques (à garder en tête)

- **H1 — contexte kernel — ✅ RÉSOLU (risque faible)** : grep confirme que les `sys_exofs_*` ne sont appelés QUE par le dispatch userspace ([table.rs](../../kernel/src/syscall/table.rs)/[mod.rs](../../kernel/src/syscall/mod.rs)) + tests. L'init FS kernel (mkroot/exofs_init) passe par `object_store`/`BLOB_CACHE` en interne, **jamais** par ces syscalls. Donc `caller_pid()` est toujours un process valide sur ce chemin. Combiné à la **politique d'open permissive** (tout process peut ouvrir → mint), aucun process n'est lock-out → le boot ne casse pas. Seules les ops globales (gc/quota/snapshot) exigent la cap FS_ROOT (→ init via étape E, serveurs via inherit/délégation).
- **H2 — arg `cap_rights`/`cap_token` (a2)** : devient ignoré (la cap est dans la table). NE PAS changer la signature ABI des syscalls (garder l'arg, l'ignorer) pour ne pas casser le wire. Nettoyage ABI = TIER 4/ultérieur.
- **H3 — `open_by_path`/`readdir`** font `if cap_rights != 0` (verify conditionnel). Rendre la vérif **inconditionnelle** sur l'object_id résolu (sinon bypass en passant 0).
- **H4 — fork inherit** : sans étape B, les fds/caps ne survivent pas au fork → ls/exec casseraient. B est PRÉREQUIS de tout test e2e multi-process.
- **H5 — round-trip bits** : OK (from_bits_truncate fidèle), mais `validate_external_mask`/`from_bits` ExoFS rejettent bit>16 — ne pas y passer les bits capability.
- **H6 — révocation** : `remove` au close ET au delete. Attention aux fds dupliqués (dup) : ne révoquer qu'au dernier close (refcount fd). Vérifier le modèle de refcount d'OBJECT_TABLE.
- **H7 — perf** : find_by_pid + cap_table.get par op. Acceptable (O(1) hachés). Hot path read/write : profiler si besoin.

## Ordre d'application + validation

1. Étape A (helper) → `mod captable`.
2. Étape B (fork inherit) — prérequis e2e.
3. Étape C (mint open) + E (boot grant init).
4. Étape D (router les 24 sites) — mécanique, par lots.
5. `make iso` (WSL) — corriger compile.
6. Run QEMU 60-90s **avec timer** : vérifier le boot atteint toujours « ipc_router registered » et au-delà (les serveurs ouvrent/opèrent via caps réelles). Si régression d'accès → ajuster `rights_from_open_flags` / H1.
7. Tests e2e (unit + boot) : sans cap = EPERM, avec cap = OK, après `remove` = EPERM.

## Décisions ouvertes (à confirmer si besoin, sinon défaut)

- **Défaut** : politique d'open permissive (tout chemin ouvrable, mint des droits selon flags) — durcissement = TIER 1. C'est ce qui évite de casser le boot tout en rendant le MÉCANISME réel (mint+vérif+révocation).
- `FS_ROOT_OBJECT_ID = 0` (ou magic dédié) pour les ops globales.
