# CORR-61 — ExoCordon : intégration dans le chemin IPC kernel (CRITIQUE)

**Source :** Audit Claude3 (BUG-S2, P0)  
**Fichiers :** `kernel/src/ipc/channel/` (ou équivalent), `kernel/src/security/ipc_policy.rs` (à créer)  
**Impact :** La politique d'isolation Ring 1 est actuellement contournable par syscall direct  
**Priorité :** Phase 0 — BLOQUANT sécurité

---

## Constat exact

`ExoCordon::check_ipc()` est uniquement appelé depuis `servers/ipc_router/src/main.rs`
au niveau du routage Ring 1. Le chemin kernel direct (syscalls `SYS_EXO_IPC_SEND` /
`SYS_EXO_IPC_CALL`) dans `kernel/src/syscall/` ne consulte **jamais** ExoCordon.

```bash
# Résultat vérifié sur le code réel :
grep -r "check_ipc\|exocordon" kernel/src/ipc/ --include="*.rs"
# → zéro résultat
```

Un processus Ring 1 ou Ring 3 peut donc appeler `sys_exo_ipc_send(src_pid, dst_pid, ...)`
directement vers n'importe quel endpoint sans passer par ipc_router, contournant
totalement le DAG d'autorité d'ExoCordon.

---

## Architecture de la correction

La vérification doit vivre en Ring 0 (kernel), pas en Ring 1 (serveur).
Deux approches possibles :

### Approche A — Module security/ipc_policy.rs (recommandée)

Créer un module léger dans le kernel qui réimplémente le check ExoCordon
en Ring 0, avec accès direct aux PIDs des processus appelants :

```rust
// kernel/src/security/ipc_policy.rs — NOUVEAU FICHIER

use crate::process::types::Pid;

/// Résultat d'une vérification de politique IPC.
#[derive(Debug, PartialEq, Eq)]
pub enum IpcPolicyResult {
    Allowed,
    Denied,
    /// PID source ou destination non répertorié — décision conservatrice.
    UnknownService,
}

/// Table d'autorisation statique : miroir kernel du DAG ExoCordon Ring 1.
///
/// RÈGLE : toute modification d'ExoCordon Ring 1 doit être répercutée ici.
/// Ces deux tables doivent rester synchronisées — envisager un crate partagé
/// à long terme (Phase 3.2).
static KERNEL_IPC_POLICY: &[(u32, u32)] = &[
    // (src_pid, dst_pid) autorisés
    // À synchroniser avec AUTHORIZED_GRAPH dans exocordon.rs
    (1, 2),   // init → ipc_broker
    (1, 3),   // init → memory_server
    (1, 4),   // init → vfs_server
    (4, 5),   // vfs → crypto
    (7, 4),   // network → vfs
    (6, 9),   // device → virtio_block
    (6, 10),  // device → virtio_net
    // NOTE : mettre à jour en même temps que exocordon.rs AUTHORIZED_GRAPH
];

/// Vérifie si un IPC direct (sans passer par ipc_router) est autorisé.
///
/// Appelé depuis le kernel avant toute livraison de message IPC.
/// Les communications passant par ipc_router (PID 2) sont toujours autorisées
/// car ipc_router applique ExoCordon Ring 1 pour le routage fin.
pub fn check_direct_ipc(src: Pid, dst: Pid) -> IpcPolicyResult {
    let src_raw = src.0;
    let dst_raw = dst.0;

    // ipc_router (PID 2) peut envoyer à n'importe qui — il est le courtier
    if src_raw == 2 { return IpcPolicyResult::Allowed; }

    // init_server (PID 1) accès complet (superviseur)
    if src_raw == 1 { return IpcPolicyResult::Allowed; }

    // Vérification de la table statique
    for &(allowed_src, allowed_dst) in KERNEL_IPC_POLICY {
        if src_raw == allowed_src && dst_raw == allowed_dst {
            return IpcPolicyResult::Allowed;
        }
    }

    IpcPolicyResult::Denied
}
```

### Intégration dans le syscall handler IPC

```rust
// kernel/src/syscall/handlers/ipc.rs (ou équivalent) — dans sys_exo_ipc_send

pub fn sys_exo_ipc_send(src_pid: Pid, dst_pid: Pid, ...) -> SyscallResult {
    // Vérification de politique AVANT tout traitement
    // CORR-61 : ExoCordon must be checked at Ring 0 level
    use crate::security::ipc_policy::{check_direct_ipc, IpcPolicyResult};

    match check_direct_ipc(src_pid, dst_pid) {
        IpcPolicyResult::Allowed => {},
        IpcPolicyResult::Denied => {
            // Log audit ExoLedger
            crate::security::exoledger::exo_ledger_append(
                exoledger::ActionTag::IpcDenied {
                    src: src_pid.0,
                    dst: dst_pid.0,
                }
            );
            return Err(SyscallError::PermissionDenied);
        }
        IpcPolicyResult::UnknownService => {
            // Service non répertorié = refus conservateur
            return Err(SyscallError::PermissionDenied);
        }
    }

    // ... suite du traitement IPC
}
```

---

## Synchronisation Ring 0 / Ring 1

**Problème à long terme :** deux tables doivent rester synchronisées.

**Solution Phase 3.2 :** extraire la politique dans un crate partagé
`libs/exo-ipc-policy/src/lib.rs` compilé à la fois dans le kernel et dans ipc_router.
Le crate doit être `no_std`, sans `alloc`.

Pour l'instant (Phase 0), ajouter un `const _: ()` de vérification de cohérence :
```rust
// ipc_policy.rs — vérification compile-time de taille de la table
const _: () = assert!(
    KERNEL_IPC_POLICY.len() == 7,
    "KERNEL_IPC_POLICY doit avoir le même nombre d'arêtes que AUTHORIZED_GRAPH dans exocordon.rs (7 arêtes)"
);
```

---

## Validation

- [ ] `grep -r "check_direct_ipc" kernel/src/syscall/` → au moins un hit
- [ ] Test : processus non autorisé tente IPC direct → `PermissionDenied`
- [ ] Test : processus via ipc_router → toujours autorisé
- [ ] Test : init_server → toujours autorisé
- [ ] Log ExoLedger créé pour chaque IPC refusé
