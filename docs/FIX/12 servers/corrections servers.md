# Corrections Techniques Détaillées - Modules Servers Exo-OS

Ce document fournit les corrections de code exactes pour chaque problème identifié dans l'analyse des incohérences.

---

## 1. Correction : `init_server/Cargo.toml`

### Fichier : `/workspace/servers/init_server/Cargo.toml`

**Avant :**
```toml
[package]
name              = "exo-init-server"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
description       = "Exo-OS server: init_server (bare-metal no_std)"

[[bin]]
name = "exo-init-server"
path = "src/main.rs"

[dependencies]
spin.workspace = true
```

**Après :**
```toml
[package]
name              = "exo-init-server"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
description       = "Exo-OS server: init_server (bare-metal no_std)"

[[bin]]
name = "exo-init-server"
path = "src/main.rs"

[dependencies]
spin.workspace = true
exo-syscall-abi = { path = "../syscall_abi" }
```

---

## 2. Correction : `exo_shield/Cargo.toml` - Nommage

### Fichier : `/workspace/servers/exo_shield/Cargo.toml`

**Avant :**
```toml
[package]
name              = "exo-exo-shield"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
description       = "Exo-OS server: exo_shield (bare-metal no_std)"

[[bin]]
name = "exo-exo-shield"
path = "src/main.rs"
```

**Après :**
```toml
[package]
name              = "exo-shield"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
description       = "Exo-OS server: exo_shield (bare-metal no_std)"

[[bin]]
name = "exo-shield"
path = "src/main.rs"
```

---

## 3. Correction : `device_server::handle_fault` - Vérification de permission

### Fichier : `/workspace/servers/device_server/src/main.rs`

**Avant :**
```rust
fn handle_fault(&mut self, payload: &[u8]) -> DeviceReply {
    let driver_pid = match read_u32(payload, 0) {
        Ok(value) => value,
        Err(err) => return DeviceReply::error(err),
    };
    let fault_code = match read_u32(payload, 4) {
        Ok(value) => value,
        Err(err) => return DeviceReply::error(err),
    };
    let value0 = match read_u64(payload, 8) {
        Ok(value) => value,
        Err(err) => return DeviceReply::error(err),
    };
    let value1 = match read_u64(payload, 16) {
        Ok(value) => value,
        Err(err) => return DeviceReply::error(err),
    };

    self.iommu
        .report_fault(driver_pid, fault_code, value0, value1);
    // ...
}
```

**Après :**
```rust
fn handle_fault(&mut self, sender_pid: u32, payload: &[u8]) -> DeviceReply {
    // Vérification : seul init_server (PID 1) ou le driver lui-même peut signaler une fault
    if sender_pid != 1 {
        // Vérifier si sender_pid est un driver enregistré
        let is_driver = self.registry.is_registered_driver(sender_pid);
        if !is_driver {
            return DeviceReply::error(exo_syscall_abi::EPERM);
        }
    }

    let driver_pid = match read_u32(payload, 0) {
        Ok(value) => value,
        Err(err) => return DeviceReply::error(err),
    };
    let fault_code = match read_u32(payload, 4) {
        Ok(value) => value,
        Err(err) => return DeviceReply::error(err),
    };
    let value0 = match read_u64(payload, 8) {
        Ok(value) => value,
        Err(err) => return DeviceReply::error(err),
    };
    let value1 = match read_u64(payload, 16) {
        Ok(value) => value,
        Err(err) => return DeviceReply::error(err),
    };

    self.iommu
        .report_fault(driver_pid, fault_code, value0, value1);
    // ...
}
```

**Note:** Nécessite d'ajouter la méthode `is_registered_driver` dans `PciRegistry`.

---

## 4. Correction : `vfs_server::handle_mount` - Vérification de permission

### Fichier : `/workspace/servers/vfs_server/src/main.rs`

**Avant :**
```rust
fn handle_mount(payload: &[u8]) -> VfsReply {
    // payload[0] = fstype u8, payload[1..5] = flags u32 LE,
    // payload[5..13] = root_blob u64 LE, payload[13..] = chemin null-terminated
    if payload.len() < 14 {
        return VfsReply {
            status: -22,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        }; // -EINVAL
    }
    // ...
}
```

**Après :**
```rust
fn handle_mount(sender_pid: u32, payload: &[u8]) -> VfsReply {
    // Seul init_server (PID 1) peut monter/démonter des filesystems
    if sender_pid != 1 {
        return VfsReply {
            status: syscall::EPERM,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        };
    }

    // payload[0] = fstype u8, payload[1..5] = flags u32 LE,
    // payload[5..13] = root_blob u64 LE, payload[13..] = chemin null-terminated
    if payload.len() < 14 {
        return VfsReply {
            status: syscall::EINVAL,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        };
    }
    // ...
}
```

---

## 5. Correction : `vfs_server::handle_umount` - Vérification de permission

### Fichier : `/workspace/servers/vfs_server/src/main.rs`

**Avant :**
```rust
fn handle_umount(payload: &[u8]) -> VfsReply {
    let path_len = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    if path_len == 0 {
        return VfsReply {
            status: -22,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        };
    }
    // ...
}
```

**Après :**
```rust
fn handle_umount(sender_pid: u32, payload: &[u8]) -> VfsReply {
    // Seul init_server (PID 1) peut monter/démonter des filesystems
    if sender_pid != 1 {
        return VfsReply {
            status: syscall::EPERM,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        };
    }

    let path_len = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    if path_len == 0 {
        return VfsReply {
            status: syscall::EINVAL,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        };
    }
    // ...
}
```

**Note:** La fonction `handle_request` doit être mise à jour pour passer `sender_pid` :

```rust
fn handle_request(req: &VfsRequest) -> VfsReply {
    match req.msg_type {
        VFS_MOUNT => handle_mount(req.sender_pid, &req.payload),
        VFS_RESOLVE => handle_resolve(&req.payload),
        VFS_OPEN => handle_open(&req.payload),
        VFS_UMOUNT => handle_umount(req.sender_pid, &req.payload),
        _ => VfsReply {
            status: syscall::EINVAL,
            blob_id: 0,
            fd: -1,
            _pad: [0; 40],
        },
    }
}
```

---

## 6. Correction : `crypto_server::ks_insert` - Quota par PID

### Fichier : `/workspace/servers/crypto_server/src/main.rs`

**Ajouter avant `ks_insert` :**
```rust
/// Compter le nombre de clés appartenant à un PID donné.
fn ks_count_for_pid(owner_pid: u32) -> usize {
    let slots = unsafe { &*KS_SLOTS.0.get() };
    let mut count = 0;
    for slot in slots.iter() {
        if slot.handle.load(Ordering::Relaxed) != 0
            && slot.owner_pid.load(Ordering::Relaxed) == owner_pid
        {
            count += 1;
        }
    }
    count
}

/// Quota maximum de clés par PID.
const KS_MAX_PER_PID: usize = 4;
```

**Avant :**
```rust
fn ks_insert(key: &[u8; KS_KEY_SIZE], key_type: u8, owner_pid: u32) -> u32 {
    let slots = unsafe { &mut *KS_SLOTS.0.get() };
    for slot in slots.iter_mut() {
        if slot.handle.load(Ordering::Relaxed) == 0 {
            let h = KS_HANDLE_CTR.fetch_add(1, Ordering::Relaxed);
            let h = if h == 0 {
                KS_HANDLE_CTR.fetch_add(1, Ordering::Relaxed)
            } else {
                h
            };
            slot.key.copy_from_slice(key);
            slot.key_type.store(key_type, Ordering::Relaxed);
            slot.owner_pid.store(owner_pid, Ordering::Relaxed);
            slot.created_at.store(read_tsc(), Ordering::Relaxed);
            slot.handle.store(h, Ordering::Release);
            return h;
        }
    }
    0
}
```

**Après :**
```rust
fn ks_insert(key: &[u8; KS_KEY_SIZE], key_type: u8, owner_pid: u32) -> u32 {
    // Vérifier le quota par PID
    if ks_count_for_pid(owner_pid) >= KS_MAX_PER_PID {
        return 0; // Quota atteint
    }

    let slots = unsafe { &mut *KS_SLOTS.0.get() };
    for slot in slots.iter_mut() {
        if slot.handle.load(Ordering::Relaxed) == 0 {
            let h = KS_HANDLE_CTR.fetch_add(1, Ordering::Relaxed);
            let h = if h == 0 {
                KS_HANDLE_CTR.fetch_add(1, Ordering::Relaxed)
            } else {
                h
            };
            slot.key.copy_from_slice(key);
            slot.key_type.store(key_type, Ordering::Relaxed);
            slot.owner_pid.store(owner_pid, Ordering::Relaxed);
            slot.created_at.store(read_tsc(), Ordering::Relaxed);
            slot.handle.store(h, Ordering::SeqCst); // SeqCst pour cohérence
            return h;
        }
    }
    0
}
```

---

## 7. Correction : `scheduler_server::handle_realtime_admit` - Validation period_us

### Fichier : `/workspace/servers/scheduler_server/src/main.rs`

**Avant :**
```rust
fn handle_realtime_admit(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
    let tid = match read_u32(payload, 0) {
        Ok(0) => sender_pid,
        Ok(value) => value,
        Err(err) => return SchedulerReply::error(err),
    };
    let runtime_us = match read_u32(payload, 4) {
        Ok(value) => value,
        Err(err) => return SchedulerReply::error(err),
    };
    let period_us = match read_u32(payload, 8) {
        Ok(value) => value,
        Err(err) => return SchedulerReply::error(err),
    };
    // ...

    match self.realtime.admit(tid, runtime_us, period_us) {
        // ...
    }
}
```

**Après :**
```rust
fn handle_realtime_admit(&mut self, sender_pid: u32, payload: &[u8]) -> SchedulerReply {
    let tid = match read_u32(payload, 0) {
        Ok(0) => sender_pid,
        Ok(value) => value,
        Err(err) => return SchedulerReply::error(err),
    };
    let runtime_us = match read_u32(payload, 4) {
        Ok(value) => value,
        Err(err) => return SchedulerReply::error(err),
    };
    let period_us = match read_u32(payload, 8) {
        Ok(value) => value,
        Err(err) => return SchedulerReply::error(err),
    };

    // Validation : period_us doit être > 0 et runtime_us <= period_us
    if period_us == 0 || runtime_us == 0 || runtime_us > period_us {
        return SchedulerReply::error(exo_syscall_abi::EINVAL);
    }

    // ...

    match self.realtime.admit(tid, runtime_us, period_us) {
        // ...
    }
}
```

---

## 8. Correction : Uniformiser les codes d'erreur dans `vfs_server`

### Fichier : `/workspace/servers/vfs_server/src/main.rs`

**Remplacer toutes les occurrences de valeurs littérales par des constantes :**

**Avant :**
```rust
return VfsReply {
    status: -22,
    blob_id: 0,
    fd: -1,
    _pad: [0; 40],
};
```

**Après :**
```rust
return VfsReply {
    status: syscall::EINVAL,
    blob_id: 0,
    fd: -1,
    _pad: [0; 40],
};
```

**Autres remplacements nécessaires :**
```rust
// -2 → ENOENT
status: -2  →  status: syscall::ENOENT

// -22 → EINVAL
status: -22  →  status: syscall::EINVAL

// -28 → ENOSPC
status: -28  →  status: syscall::ENOSPC
```

---

## 9. Correction : Ajouter des timeouts IPC aux serveurs sans timeout

### 9.1. `network_server/src/main.rs`

**Avant :**
```rust
#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_endpoint();
    let mut request = NetworkRequest::zeroed();

    loop {
        match recv_request(&mut request) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => continue,
        }

        let reply = if request.msg_type == NETWORK_MSG_HEARTBEAT {
            send_heartbeat()
        } else {
            dispatch(&request)
        };

        let _ = send_reply(request.sender_pid, &reply);
    }
}
```

**Après :**
```rust
const IPC_RECV_TIMEOUT_MS: u64 = 5_000;
const IPC_FLAG_TIMEOUT: u64 = exo_syscall_abi::IPC_FLAG_TIMEOUT;
const ETIMEDOUT: i64 = exo_syscall_abi::ETIMEDOUT;

static IPC_RECV_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static RECV_ERRORS: AtomicU32 = AtomicU32::new(0);

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_endpoint();
    let mut request = NetworkRequest::zeroed();

    loop {
        // Utiliser syscall direct avec timeout au lieu de recv_request bloquant
        let r = unsafe {
            exo_syscall_abi::syscall3(
                exo_syscall_abi::SYS_IPC_RECV,
                &mut request as *mut NetworkRequest as u64,
                core::mem::size_of::<NetworkRequest>() as u64,
                IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
            )
        };

        if r == ETIMEDOUT {
            IPC_RECV_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            // Maintenance périodique peut être faite ici
            continue;
        }
        if r < 0 {
            RECV_ERRORS.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        let reply = if request.msg_type == NETWORK_MSG_HEARTBEAT {
            send_heartbeat()
        } else {
            dispatch(&request)
        };

        let _ = send_reply(request.sender_pid, &reply);
    }
}
```

### 9.2. Appliquer la même correction à :
- `memory_server/src/main.rs`
- `scheduler_server/src/main.rs`
- `device_server/src/main.rs`

---

## 10. Correction : Améliorer les panic handlers

### Fichier : Tous les `main.rs` des serveurs

**Avant :**
```rust
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
```

**Après (exemple pour network_server) :**
```rust
use core::sync::atomic::{AtomicBool, AtomicU64};

static PANIC_OCCURRED: AtomicBool = AtomicBool::new(false);
static PANIC_CODE: AtomicU64 = AtomicU64::new(0);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Marquer qu'un panic s'est produit (pour init_server)
    if !PANIC_OCCURRED.swap(true, Ordering::SeqCst) {
        // Encoder une information minimale sur le panic
        let code = if info.location().is_some() {
            1 // Panic avec location
        } else {
            2 // Panic sans location
        };
        PANIC_CODE.store(code, Ordering::SeqCst);

        // Tenter de notifier init_server via IPC (non-bloquant)
        // Note: ceci est optionnel et peut échouer
        unsafe {
            let msg = (info.message(), info.location());
            // Envoi non-bloquant - ignorer les erreurs
            let _ = exo_syscall_abi::syscall6(
                exo_syscall_abi::SYS_IPC_SEND,
                1, // init_server PID
                &msg as *const _ as u64,
                core::mem::size_of_val(&msg) as u64,
                0,
                0,
                0,
            );
        }
    }

    // Halt CPU en attendant le redémarrage par init_server
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
```

---

## 11. Correction : Standardiser l'enregistrement IPC avec endpoint_id

### 11.1. `memory_server/src/ipc_bridge.rs` (ou fichier équivalent)

**Créer/Modifier la fonction `register_endpoint` :**

```rust
const MEMORY_SERVER_PID: u32 = 5; // À définir selon la convention

pub fn register_endpoint() -> Result<(), i64> {
    let name = b"memory_server";
    let rc = unsafe {
        exo_syscall_abi::syscall3(
            exo_syscall_abi::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            MEMORY_SERVER_PID as u64,
        )
    };
    if rc < 0 {
        Err(rc)
    } else {
        Ok(())
    }
}
```

### 11.2. Appliquer la même correction à :
- `network_server/src/socket/api.rs` → `register_endpoint()`
- `scheduler_server/src/protocol.rs` → `register_endpoint()`
- `device_server/src/protocol.rs` → `register_endpoint()`

---

## 12. Correction : Ajouter des compteurs de statistiques

### Exemple pour `network_server/src/main.rs`

**Ajouter les variables globales :**
```rust
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUESTS_OK: AtomicU64 = AtomicU64::new(0);
static REQUESTS_ERR: AtomicU64 = AtomicU64::new(0);
static IPC_RECV_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static RECV_ERRORS: AtomicU32 = AtomicU32::new(0);
```

**Mettre à jour la boucle principale :**
```rust
loop {
    let r = unsafe {
        exo_syscall_abi::syscall3(
            exo_syscall_abi::SYS_IPC_RECV,
            &mut request as *mut NetworkRequest as u64,
            core::mem::size_of::<NetworkRequest>() as u64,
            IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
        )
    };

    if r == ETIMEDOUT {
        IPC_RECV_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
        continue;
    }
    if r < 0 {
        RECV_ERRORS.fetch_add(1, Ordering::Relaxed);
        continue;
    }

    REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);

    let reply = if request.msg_type == NETWORK_MSG_HEARTBEAT {
        send_heartbeat()
    } else {
        dispatch(&request)
    };

    if reply.status == 0 {
        REQUESTS_OK.fetch_add(1, Ordering::Relaxed);
    } else {
        REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
    }

    let _ = send_reply(request.sender_pid, &reply);
}
```

---

## Checklist Finale de Vérification

Après avoir appliqué toutes les corrections, vérifier :

- [ ] `cargo check --workspace` compile sans erreurs
- [ ] Tous les serveurs ont un timeout IPC configuré
- [ ] Toutes les fonctions sensibles vérifient `sender_pid`
- [ ] Tous les codes d'erreur utilisent `exo_syscall_abi::E*`
- [ ] Les compteurs de statistiques sont présents dans chaque serveur
- [ ] Les panic handlers incluent une notification à init_server
- [ ] Chaque serveur s'enregistre avec un endpoint_id explicite
- [ ] Le keystore crypto a un quota par PID
- [ ] Les validations d'entrée (period_us, etc.) sont présentes

---

## Conclusion

Ces corrections techniques détaillées doivent être appliquées méthodiquement, en testant chaque changement individuellement. Après application, une revue de code complète et des tests d'intégration sont recommandés pour valider la stabilité du système.