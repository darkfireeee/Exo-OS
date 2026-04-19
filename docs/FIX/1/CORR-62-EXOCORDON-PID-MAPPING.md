# CORR-62 — ExoCordon : correction du mapping PID ↔ ServiceId

**Source :** Audit Claude3 (BUG-S3, P0)  
**Fichier :** `servers/ipc_router/src/exocordon.rs`  
**Priorité :** Phase 0

---

## Constat exact

### Enum ServiceId (valeurs ordinales)
```rust
enum ServiceId {
    Init = 1,
    Memory = 2,   // ← ordinal enum = 2
    Vfs = 3,
    Crypto = 4,
    Device = 5,
    Network = 6,
    Scheduler = 7,
    VirtioBlock = 8,
    VirtioNet = 9,
    ExoShield = 10,
}
```

### service_id_of() — mapping PID → ServiceId
```rust
fn service_id_of(raw: Pid) -> Option<ServiceId> {
    match raw {
        1 => Some(ServiceId::Init),
        3 => Some(ServiceId::Vfs),       // PID 3 → Vfs
        4 => Some(ServiceId::Crypto),    // PID 4 → Crypto
        5 => Some(ServiceId::Memory),    // PID 5 → Memory (mais Memory enum = 2 !)
        6 => Some(ServiceId::Device),
        ...
        _ => None,
    }
}
// PID 2 → None  ← ipc_broker non mappé !
```

### Séquence de démarrage Ring 1 V4 (SERVICES dans init_server)
```
PID 1  = init_server (kernel)
PID 2  = ipc_router  (premier spawné)
PID 3  = memory_server
PID 4  = vfs_server
PID 5  = crypto_server
PID 6  = device_server
PID 7  = network_server
PID 8  = scheduler_server
PID 9  = virtio_drivers
PID 10 = exo_shield
```

### Problèmes identifiés
1. `ServiceId::Memory` (ordinal=2) ≠ PID de memory_server (PID 3)
2. PID 2 (ipc_router) → `None` dans service_id_of — UnknownService
3. Memory mappé sur PID 5 (qui est crypto_server en réalité)
4. `ServiceId::Vfs` mappé sur PID 3 (qui est memory_server en réalité)

Résultat : toutes les règles AUTHORIZED_GRAPH impliquant Memory (Init→Memory)
échouent avec UnknownService ou pointent vers le mauvais service.

---

## Correction

### Option A (recommandée) — aligner les ordinals enum sur les PIDs réels

```rust
// exocordon.rs — APRÈS correction
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum ServiceId {
    Init         = 1,
    IpcBroker    = 2,   // ipc_router — PID 2
    Memory       = 3,   // memory_server — PID 3 ← corrigé
    Vfs          = 4,   // vfs_server — PID 4 ← corrigé
    Crypto       = 5,   // crypto_server — PID 5 ← corrigé
    Device       = 6,   // device_server — PID 6 ← corrigé
    Network      = 7,   // network_server — PID 7 ← corrigé
    Scheduler    = 8,   // scheduler_server — PID 8 ← corrigé
    VirtioBlock  = 9,   // virtio_drivers — PID 9
    VirtioNet    = 10,  // virtio_net — PID 10 (si séparé)
    ExoShield    = 11,  // exo_shield — PID 11 ← corrigé (était 10)
}

fn service_id_of(raw: Pid) -> Option<ServiceId> {
    match raw {
        1  => Some(ServiceId::Init),
        2  => Some(ServiceId::IpcBroker),
        3  => Some(ServiceId::Memory),
        4  => Some(ServiceId::Vfs),
        5  => Some(ServiceId::Crypto),
        6  => Some(ServiceId::Device),
        7  => Some(ServiceId::Network),
        8  => Some(ServiceId::Scheduler),
        9  => Some(ServiceId::VirtioBlock),
        10 => Some(ServiceId::VirtioNet),
        11 => Some(ServiceId::ExoShield),
        _  => None,
    }
}
```

### Mettre à jour AUTHORIZED_GRAPH en conséquence

```rust
static AUTHORIZED_GRAPH: [AuthEdge; 7] = [
    AuthEdge::new(ServiceId::Init,    ServiceId::Memory,      4, 10_000),
    AuthEdge::new(ServiceId::Init,    ServiceId::Vfs,         4, 10_000),
    AuthEdge::new(ServiceId::Vfs,     ServiceId::Crypto,      2, 50_000),
    AuthEdge::new(ServiceId::Network, ServiceId::Vfs,         2, 100_000),
    AuthEdge::new(ServiceId::Device,  ServiceId::VirtioBlock, 1, 1_000_000),
    AuthEdge::new(ServiceId::Device,  ServiceId::VirtioNet,   1, 1_000_000),
    // IpcBroker peut communiquer avec tous — ajouter si besoin de contrôle fin
    // AuthEdge::new(ServiceId::IpcBroker, ServiceId::Memory, 4, u64::MAX),
];
```

### Ajouter une assertion compile-time de cohérence avec init_server

Le nombre de services dans SERVICES (init_server) doit correspondre au nombre de
ServiceId mappés. Ajouter dans exocordon.rs :

```rust
// Doit correspondre au nombre de services dans init_server::SERVICES + 1 (init lui-même)
const _: () = assert!(
    ServiceId::ExoShield as u8 == 11,
    "ExoShield doit être le dernier ServiceId (SRV-04) — vérifier l'ordre de spawn Ring 1 V4"
);
```

---

## Impact sur CORR-61

La table `KERNEL_IPC_POLICY` dans `kernel/src/security/ipc_policy.rs` (CORR-61)
doit utiliser les **PID réels** (pas les ordinals enum). Elle est déjà correcte si
définie en termes de numéros PID. Vérifier la cohérence après cette correction.

---

## Validation

- [ ] Test : `service_id_of(2)` → `Some(ServiceId::IpcBroker)` (non plus `None`)
- [ ] Test : `service_id_of(3)` → `Some(ServiceId::Memory)` (non plus `Vfs`)
- [ ] Test : Init→Memory IPC via ipc_router → `Allowed` (non plus `UnknownService`)
- [ ] Test : check_ipc(PID1, PID3) → `Ok(())` (init peut parler à memory_server)
- [ ] `cargo test --package ipc_router` → tous les tests passent
