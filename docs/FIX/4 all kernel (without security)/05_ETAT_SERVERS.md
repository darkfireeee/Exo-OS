# ExoOS — État des Servers Ring1 et Gaps Restants
## Commit de référence : `c4239ed1`

---

## Vue d'ensemble

| Server | PID | Lignes | État | Bloquant principal |
|--------|-----|--------|------|--------------------|
| `init_server`     | 1 | 303 | 🟡 Partiel | fork/execve (P0-01/02) |
| `ipc_router`      | 2 | 240 | 🟡 Partiel | numéros syscall (P0-03) + IPC ENOSYS (P0-05) |
| `vfs_server`      | 3 | 400 | 🟡 Partiel | fs_bridge (P0-04) + fork/execve (P0-01/02) |
| `crypto_server`   | 4 | 219 | 🟡 Partiel | fork/execve (P0-01/02) |
| `memory_server`   | 5 | 16  | 🔴 Stub pur | `loop {}` — aucune implémentation |
| `device_server`   | 6 | 16  | 🔴 Stub pur | `loop {}` — aucune implémentation |
| `network_server`  | 7 | 16  | 🔴 Stub pur | `loop {}` — aucune implémentation |
| `scheduler_server`| 8 | 16  | 🔴 Stub pur | `loop {}` — aucune implémentation |
| `exo_shield`      | 9 | 16  | 🔴 Stub pur | `loop {}` — aucune implémentation |

---

## Analyse détaillée par server

### `init_server` (PID 1) — 303 lignes

**Ce qui est implémenté :**
- Séquence de boot Ring1 V4 canonique (`boot_sequence.rs`) : démarrage ordonné de 9 services
- `spawn_service()` via `fork + execve`
- Supervision SIGCHLD : détection crash, relance avec délai exponentiel (max 32s)
- Gestion `SIGTERM` → arrêt ordonné de tous les services
- Dépendances inter-services (`dependency.rs`)
- Tableau `SERVICES[9]` complet avec tous les chemins binaires

**Ce qui bloque :**
- `fork()` → EFAULT (P0-01 : AddressSpaceCloner non enregistré)
- `execve()` → ENOSYS (P0-02 : ElfLoader non enregistré)
- Sans P0-01/02, `boot_services()` échoue au premier `spawn_service()`

**Gaps spécifiques :**
```
servers/init_server/src/dependency.rs
→ `dependencies_satisfied()` check valide
→ `ready_timeout_ms()` : timeouts codés en dur (ipc_router=500ms, reste=2000ms)
   → à lire depuis un fichier de config une fois fs opérationnel
```

---

### `ipc_router` (PID 2) — 240 lignes

**Ce qui est implémenté :**
- Boucle receive → dispatch fonctionnelle
- Registry en mémoire (table de 64 services max, pas d'allocation heap)
- Heartbeat : répond à `IPC_MSG_HEARTBEAT` avec son PID
- Security gate via `exocordon::check_ipc()` (module de politique IPC)
- Forward de messages vers endpoints

**Ce qui bloque :**
1. **P0-03** : utilise `SYS_IPC_REGISTER=300` → kernel interprète comme `SYS_EXO_IPC_SEND`
2. **P0-05** : `sys_exo_ipc_recv` → ENOSYS → la boucle de réception ne reçoit jamais rien
3. **P2-05** : `SYS_IPC_SEND=302` → kernel interprète comme `SYS_EXO_IPC_RECV_NB`

**Gap spécifique :**
```
servers/ipc_router/src/main.rs:52
static REGISTRY: Registry — table fixe de MAX_SERVICES=64 entrées
→ Limite documentée, non bloquante en Phase 1
→ À passer en allocation dynamique si > 64 services
```

---

### `vfs_server` (PID 3) — 400 lignes

**Ce qui est implémenté :**
- Montage ExoFS sur `/` (via `SYS_EXOFS_*`)
- Pseudo-filesystems : `/proc`, `/sys`, `/dev` (stubs de montage)
- Résolution de chemins (`VFS_RESOLVE`)
- Table de montages : 32 points max
- `flush_exofs_mount()` appelé correctement (CORR-67 ✅)

**Ce qui bloque :**
- **P0-04** : `sys_read/write/open` → ENOSYS → impossible d'accéder aux fichiers
- **P0-01/02** : ne peut pas être lancé par `init_server`

**Gaps spécifiques :**
```
servers/vfs_server/src/main.rs:
→ VFS_OPEN (msg_type=3) : stub — retourne ENOSYS directement
  → À câbler vers SYS_EXOFS_OBJECT_OPEN quand fs_bridge est opérationnel
→ /proc et /sys : montés mais aucun contenu réel (pas de readdir, pas de read)
→ Pas de gestion des montages NFS/FUSE (prévu Phase 3)
```

---

### `crypto_server` (PID 4) — 219 lignes

**Ce qui est implémenté :**
- TLS 1.3 simplifié (négociation partielle)
- Keystore en mémoire (ring d'entrées BLAKE3-hashées)
- Handlers pour 5 opérations : DERIVE_KEY, RANDOM, ENCRYPT, DECRYPT, HASH

**Ce qui bloque :**
- **P0-01/02** : ne peut pas être lancé par `init_server`
- **P0-05** : IPC → ENOSYS → les clients ne peuvent pas envoyer de requêtes

**Gaps spécifiques :**
```
servers/crypto_server/src/main.rs:
→ keystore : stockage en mémoire volatile uniquement
  → pas de persistence entre redémarrages (à stocker dans ExoFS chiffré)
→ CRYPTO_ENCRYPT/DECRYPT : utilisent XChaCha20-Poly1305 kernel-side ✓
  mais le transport des clés via IPC n'est pas finalisé
→ TLS : nego partielle — pas de vérification de certificat (stub accept-all)
```

---

### `memory_server` (PID 5) — 16 lignes — Stub pur

**Code actuel :**
```rust
#[no_mangle]
pub extern "C" fn _start() -> ! { loop {} }
```

**Rôle prévu (GI-04 §3.2) :**
- Gestionnaire de régions mémoire Ring1
- Délégation des syscalls `mmap/munmap/brk` depuis les processus Ring3
- Policy de quotas mémoire par processus (RLIMIT_AS)
- Interface vers le buddy allocator via `SYS_EXO_MEM_SHARE/REVOKE`

**Implémentation minimale requise :**
```rust
// servers/memory_server/src/main.rs — squelette minimum

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 1. Enregistrer l'endpoint
    let _ = unsafe {
        syscall(SYS_EXO_IPC_CREATE, b"memory_server".as_ptr() as u64, 13, 5)
    };

    // 2. Boucle de service
    let mut msg = [0u8; 128];
    loop {
        let r = unsafe {
            syscall(SYS_EXO_IPC_RECV, msg.as_mut_ptr() as u64, 128, 0)
        };
        if r < 0 { continue; }

        // Dispatcher selon msg_type
        let msg_type = u32::from_le_bytes(msg[0..4].try_into().unwrap());
        match msg_type {
            MEMORY_MSG_MMAP   => handle_mmap(&msg),
            MEMORY_MSG_MUNMAP => handle_munmap(&msg),
            MEMORY_MSG_BRK    => handle_brk(&msg),
            _                 => {},
        }
    }
}
```

---

### `device_server` (PID 6) — 16 lignes — Stub pur

**Rôle prévu (GI-03 §5) :**
- Interface Ring1 vers les drivers GI-03 du kernel
- Arbitrage des accès MMIO/DMA entre processus Ring3
- Enumération PCI (expose une liste de devices aux processus)

**Dépendances bloquantes :**
- Nécessite `ipc_router` opérationnel (P0-03/05)
- Nécessite `SYS_MMIO_MAP/UNMAP` (530-531, déjà dans le kernel ✓)

---

### `network_server` (PID 7) — 16 lignes — Stub pur

**Rôle prévu :**
- Stack réseau Ring1 (virtio-net driver)
- Interface socket POSIX (SYS_SOCKET/CONNECT/BIND → ENOSYS actuellement)
- Nécessite `device_server` opérationnel

---

### `scheduler_server` (PID 8) — 16 lignes — Stub pur

**Rôle prévu :**
- Policy de scheduling Ring1 : priorités, affinités, quotas CPU
- Délégation vers le CFS kernel via IPC
- Interface `nice()`, `setpriority()`, CPU affinity

---

### `exo_shield` (PID 9) — 16 lignes — Stub pur

**Rôle prévu :**
- Interface Ring1 vers les modules kernel ExoShield (exocage, exokairos...)
- Distribution des capabilities aux processus Ring3
- Audit trail Ring1

**Note** : dépend de la refonte ExoShield en cours — ne pas implémenter avant la refonte.

---

## Ordre de priorité d'implémentation des stubs

```
Niveau 1 (débloque tout le reste) :
  → Appliquer P0-01 à P0-05 (corrections critiques)
  → Après P0 : ipc_router et init_server fonctionnent

Niveau 2 (services de base) :
  1. memory_server  — nécessaire pour mmap Ring3
  2. device_server  — nécessaire pour accès hardware Ring1

Niveau 3 (services réseau et scheduling) :
  3. network_server — après device_server
  4. scheduler_server — policy optionnelle en Phase 1

Niveau 4 (sécurité avancée) :
  5. exo_shield — après refonte ExoShield
```

---

## Checklist de validation par server

Chaque server doit passer ces tests avant d'être marqué "opérationnel" :

```
[ ] Démarrage via init_server::spawn_service() sans erreur
[ ] Enregistrement endpoint IPC réussi (SYS_EXO_IPC_CREATE → 0)
[ ] Réponse au heartbeat init_server (IPC_MSG_HEARTBEAT)
[ ] Survie au restart après SIGKILL (init_server relance en < 32s)
[ ] Aucun kernel panic sur message IPC malformé (fuzzing basic)
[ ] Réponse correcte à au moins un message métier (ex: VFS_RESOLVE pour vfs_server)
```
