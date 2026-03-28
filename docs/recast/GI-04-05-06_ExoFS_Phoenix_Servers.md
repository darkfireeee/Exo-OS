# ExoOS — Guide d'Implémentation GI-04
## ExoFS & POSIX Bridge

**Prérequis** : GI-01, GI-02  
**Produit** : `kernel/src/fs/exofs/`, `servers/vfs_server/`

---

## 1. Ordre d'Implémentation

```
Étape 1 : io/reader.rs            ← ZERO_BLOB_ID_4K ghost blob
Étape 2 : posix_bridge/truncate_kernel.rs
Étape 3 : posix_bridge/fallocate_kernel.rs
Étape 4 : posix_bridge/copy_range_kernel.rs
Étape 5 : posix_bridge/msync_kernel.rs
Étape 6 : syscall/mod.rs          ← Dispatch 500-519
Étape 7 : vfs_server/ops/         ← Toutes les opérations POSIX
Étape 8 : vfs_server/isolation.rs ← PrepareIsolation + sync_fs
```

---

## 2. io/reader.rs — Ghost Blob Pattern

```rust
// kernel/src/fs/exofs/io/reader.rs
//
// RÈGLE TL-31 : ZERO_BLOB_ID_4K → memset(0) SANS I/O disque
//
// ❌ ERREUR SILENCIEUSE : Chercher ZERO_BLOB_ID_4K dans blob_registry
//    → NotFound → retourne une erreur au lieu de zéros
//    → Le fichier sparse semble corrompu pour l'application
//
// ❌ ERREUR : blob_refcount::increment(ZERO_BLOB_ID_4K)
//    → Corrompt le système de déduplication
//    → Guard explicite : if p_blob_id != ZERO_BLOB_ID_4K
//
// RÈGLE TL-23 : checksum Blake3 vérifié AVANT décompression
//    → Décompresser des données corrompues peut crasher le décompresseur

pub fn read_p_blob(p_blob_id: ObjectId, dst: &mut [u8]) -> Result<(), ExofsError> {
    // TL-31 : Ghost blob — zéros sans aucun accès disque
    if p_blob_id == ZERO_BLOB_ID_4K {
        let fill_len = dst.len().min(EXOFS_PAGE_SIZE);
        dst[..fill_len].fill(0u8);
        return Ok(());
    }

    // Chemin normal
    let loc = blob_registry::lookup(p_blob_id)
        .ok_or(ExofsError::NotFound)?;
    let compressed = storage::read_raw(loc)?;

    // TL-23 : CHECKSUM AVANT décompression (ordre impératif)
    checksum::verify_blake3(&compressed, p_blob_id)?;

    compress::decompress_into(compressed, dst)
}
```

---

## 3. copy_range_kernel.rs — Reflink Guard

```rust
// kernel/src/fs/exofs/posix_bridge/copy_range_kernel.rs
//
// RÈGLE TL-30 : reflink = !encrypted(src) && !encrypted(dst)
// RÈGLE TL-32 : JAMAIS ZERO_BLOB_ID_4K pour page partielle
// RÈGLE CORR-47 : Vérifier quota AVANT l'opération

pub fn do_copy_file_range(
    src_obj_id: ObjectId, src_off: u64,
    dst_obj_id: ObjectId, dst_off: u64,
    len: u64,
) -> Result<CopyRangeResult, ExofsError> {
    verify_cap(src_obj_id, Rights::READ)?;
    verify_cap(dst_obj_id, Rights::WRITE)?;

    let src_size = object_table::get_size(src_obj_id)?;
    if src_off >= src_size { return Err(ExofsError::InvalidArg); }
    let actual_len = len.min(src_size.saturating_sub(src_off));

    // CORR-47 : Quota check AVANT l'opération (S-13)
    quota::check_and_reserve(dst_obj_id, actual_len)?;

    let can_reflink = !ObjectMeta::is_encrypted(src_obj_id)?
                   && !ObjectMeta::is_encrypted(dst_obj_id)?;

    let (mut total, mut reflinks) = (0u64, 0u64);

    for blob_range in extent_tree::iter_blobs(src_obj_id, src_off, actual_len) {
        let p_blob_id = blob_range.p_blob_id;

        if can_reflink {
            // Guard ZERO_BLOB_ID_4K : refcount virtuel ∞, ne JAMAIS incrémenter
            if p_blob_id != ZERO_BLOB_ID_4K {
                blob_refcount::increment(p_blob_id)?;
            }
            extent_tree::set_p_blob(dst_obj_id, dst_off + blob_range.rel_offset, p_blob_id)?;
            reflinks += 1;
        } else {
            // Objets chiffrés : copie physique
            dma_copy_blob(p_blob_id, dst_obj_id, dst_off + blob_range.rel_offset)?;
        }
        total += blob_range.len;
    }

    epoch::commit_single_op(dst_obj_id)?;
    Ok(CopyRangeResult { bytes_copied: total, reflinks_used: reflinks })
}
```

---

## 4. Syscall Dispatch 500-519 — Règle S-01

```rust
// kernel/src/fs/exofs/syscall/mod.rs
//
// RÈGLE S-01 ABSOLUE : verify_cap() en PREMIÈRE INSTRUCTION
//   de CHAQUE handler syscall ExoFS.
//
// ❌ ERREUR SILENCIEUSE : oublier verify_cap() dans un handler
//    → N'importe quel processus peut accéder à n'importe quel ObjectId
//    → Violation totale du modèle Zero Trust
//    → CI check : grep -r 'fn sys_exofs_' | grep -v 'verify_cap'
//
// ❌ ERREUR : verify_cap() après une lecture ou écriture
//    → L'accès a déjà eu lieu avant la vérification
//    → TOCTOU sur la capability elle-même

pub fn dispatch_exofs_syscall(nr: u32, args: &SyscallArgs) -> Result<u64, ExofsError> {
    match nr {
        SYS_EXOFS_READ => sys_exofs_read(args),
        SYS_EXOFS_WRITE => sys_exofs_write(args),
        SYS_EXOFS_COPY_FILE_RANGE => sys_exofs_copy_file_range(args),
        SYS_EXOFS_GET_CONTENT_HASH => sys_exofs_get_content_hash(args),
        SYS_EXOFS_EPOCH_META => Err(ExofsError::NotImplemented), // CORR-52
        _ => Err(ExofsError::InvalidSyscall),
    }
}

fn sys_exofs_read(args: &SyscallArgs) -> Result<u64, ExofsError> {
    // S-01 : verify_cap EN PREMIER — avant toute autre opération
    let obj_id = ObjectId::from_raw(args.arg0);
    verify_cap(obj_id, Rights::READ)?; // ← PREMIÈRE instruction

    let buf    = args.arg1 as *mut u8;
    let len    = args.arg2 as usize;
    let offset = args.arg3;

    // Validation de l'adresse userspace
    validate_user_ptr(buf, len)?;

    // ... lecture ...
    Ok(bytes_read as u64)
}

fn sys_exofs_get_content_hash(args: &SyscallArgs) -> Result<u64, ExofsError> {
    let obj_id = ObjectId::from_raw(args.arg0);
    // S-01 : verify_cap EN PREMIER
    verify_cap(obj_id, Rights::INSPECT_CONTENT)?;

    // S-09 : ObjectKind::Secret → jamais retourner BlobId
    if object_table::is_secret(obj_id)? {
        return Err(ExofsError::PermissionDenied);
    }

    // S-15 : TOUJOURS auditer cette opération
    audit::log_get_content_hash(obj_id, current_pid());

    let hash = hash_registry::get(obj_id)?;
    // Retourner l'ObjectId du hash (pas le contenu brut)
    Ok(hash.to_raw())
}
```

---

## 5. vfs_server/isolation.rs — PrepareIsolation Complet

```rust
// servers/vfs_server/src/isolation.rs
//
// PrepareIsolation pour vfs_server doit garantir la DURABILITÉ complète.
// Sans sync_fs + flush disque, le snapshot Phoenix peut capturer un état
// RAM incohérent avec l'état physique du disque.
//
// ORDRE IMPÉRATIF :
//   1. fd_table flush (état des descripteurs)
//   2. sync_fs (dirty pages → writeback)
//   3. disk flush (FLUSH_CACHE NVMe/SATA)
//   4. Seulement ALORS : retourner PrepareIsolationAck

pub fn prepare_isolation() -> PrepareIsolationAck {
    log::info!("vfs_server: PrepareIsolation — début sync complet");

    // Étape 1 : Fermer les pipes non-persistants (état transient)
    // Les pipes Ring 1 ne sont pas dans ExoFS → pas de durabilité requise
    // Mais les threads bloqués sur pipe doivent recevoir EPIPE
    pipe::notify_all_pending_waiters_epipe();

    // Étape 2 : Flush fd_table (état des FDs)
    fd_table::flush_all_dirty_entries();

    // Étape 3 : Sync ExoFS complet (CORR-13)
    match exofs_sync_fs() {
        Ok(()) => log::info!("vfs_server: sync_fs OK"),
        Err(e) => log::error!("vfs_server: sync_fs erreur {:?}", e),
    }

    // Étape 4 : Flush cache disque matériel (CORR-13)
    for backend in storage_backends() {
        match backend.flush_disk_cache(FLUSH_TIMEOUT_MS) {
            Ok(())  => log::debug!("vfs_server: flush {} OK", backend.name()),
            Err(e)  => log::error!("vfs_server: flush {} erreur {:?}", backend.name(), e),
        }
    }

    log::info!("vfs_server: durabilité confirmée → PrepareIsolationAck");

    PrepareIsolationAck {
        server:        ServiceName::from_bytes(b"vfs_server"),
        checkpoint_id: exofs_current_epoch(),
    }
}

const FLUSH_TIMEOUT_MS: u64 = 5000; // 5s max pour flush disque
```

---

*ExoOS — Guide d'Implémentation GI-04 : ExoFS & POSIX Bridge — Mars 2026*

---

# ExoOS — Guide d'Implémentation GI-05
## ExoPhoenix : Gel, Restore, SSR Protocol

**Prérequis** : GI-01, GI-02, GI-03, GI-04  
**Produit** : `kernel/src/exophoenix/`, `servers/exo_shield/`

---

## 1. Ordre d'Implémentation

```
Étape 1 : SSR mapping dans kernel   ← Adresse physique 0x1000000 → VMA kernel
Étape 2 : verify_ssr_magic()        ← Vérification au boot Kernel A et B
Étape 3 : handle_freeze_ipi()       ← Handler IDT 0xF3 avec timeout (CORR-33)
Étape 4 : servers : isolation.rs    ← PrepareIsolation par server (CORR-12..15)
Étape 5 : exo_shield/irq_handler.rs ← Broadcast + collecte ACKs
Étape 6 : phoenix_wake_sequence()   ← Restore + reseed + reset time base
Étape 7 : restore_sequence.rs       ← Ordre restart serveurs (CORR-35)
```

---

## 2. handle_freeze_ipi — Handler IDT 0xF3

```rust
// kernel/src/exophoenix/freeze.rs
//
// RÈGLES ABSOLUES pour handle_freeze_ipi :
//
// 1. DOIT avoir un timeout (CORR-33)
//    → Si Kernel B crashe pendant snapshot, évite deadlock infini
//
// 2. CR0.TS interprétation correcte (ERRATA-01 / GI-02) :
//    CR0.TS = 0 → FPU active dans registres → XSAVE requis
//    CR0.TS = 1 → FPU lazy (déjà dans fpu_state_ptr) → pas de XSAVE
//    if !cr0.contains(TASK_SWITCHED) → TS=0 → FPU active → XSAVE
//
// 3. Ne pas appeler de fonction bloquante (IRQs désactivées par IPI)

pub fn handle_freeze_ipi() {
    // ─── Sauvegarder l'état FPU si actif ─────────────────────────────
    let tcb = scheduler::current_tcb_mut();

    // CR0.TS = 0 = FPU active = état dans registres CPU = XSAVE requis
    if !is_cr0_ts_set() && tcb.fpu_state_ptr != 0 {
        unsafe { fpu::xsave64(tcb.fpu_state_ptr as *mut XSaveArea); }
        unsafe { set_cr0_ts(); } // CR0.TS = 1 pour indiquer lazy save fait
    }

    // ─── Écrire FREEZE_ACK dans SSR ──────────────────────────────────
    let apic_id = lapic::current_apic_id() as usize;
    write_ssr_atomic(freeze_ack_offset(apic_id), 1u64);

    // ─── Spin-wait avec timeout (CORR-33) ────────────────────────────
    let khz = BOOT_TSC_KHZ.load(Ordering::Relaxed);
    let timeout_ticks = if khz > 0 { 100 * khz } else { 300_000_000u64 };
    let start = unsafe { core::arch::x86_64::_rdtsc() };

    loop {
        let handoff = read_ssr_atomic(SSR_HANDOFF_FLAG_OFFSET);
        match handoff {
            3 | 2 => break, // B_ACTIVE ou FREEZE_ACK_ALL = reprise
            _ => {}
        }
        let elapsed = unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(start);
        if elapsed >= timeout_ticks {
            unsafe { arch::serial_write(b"[PHOENIX] freeze timeout\n"); }
            // Écrire ACK dégradé et continuer
            write_ssr_atomic(freeze_ack_offset(apic_id), 0xDEAD_ACK);
            break;
        }
        unsafe { core::arch::asm!("pause", options(nostack, nomem)); }
    }
}
```

---

## 3. phoenix_wake_sequence — Post-Restore

```rust
// kernel/src/exophoenix/restore.rs
//
// ORDRE IMPÉRATIF post-restore (CORR-12 + CORR-34) :
//
// 1. Réinitialiser la base de temps (AVANT les watchdogs IRQ)
//    → Sans reset, watchdogs voient un delta énorme (temps du gel)
//    → False positives → reset de tous les drivers
//
// 2. Reset masked_since pour les routes IRQ actives
//    → Même raison : masked_since stale depuis le gel
//
// 3. Envoyer PhoenixWakeEntropy à crypto_server (CORR-12)
//    → AVANT tout autre message IPC (crypto doit être prêt)
//    → RDRAND/RDSEED pour l'epoch_id (hardware entropy)

pub fn phoenix_wake_sequence() {
    log::info!("Phoenix wake: démarrage séquence post-restore");

    // ─── 1. Réinitialiser la base de temps (CORR-34) ──────────────────
    time::phoenix_reset_time_base();

    // ─── 2. Reset masked_since IRQ routes (CORR-12) ───────────────────
    irq_routing::reset_all_masked_since();

    // ─── 3. Générer epoch_id hardware (RDSEED préféré à RDRAND) ───────
    let epoch_id = arch::rdseed()
        .or_else(|| arch::rdrand())
        .unwrap_or_else(|| unsafe { core::arch::x86_64::_rdtsc() });
    let timestamp = unsafe { core::arch::x86_64::_rdtsc() };

    // ─── 4. Envoyer à crypto_server AVANT tout autre service ──────────
    let entropy_msg = IpcMessage {
        sender_pid:  0, // Kernel
        msg_type:    IPC_PHOENIX_WAKE_ENTROPY,
        reply_nonce: 0,
        _pad:        0,
        payload:     encode_entropy(epoch_id, timestamp),
    };
    ipc::send_priority_blocking(CRYPTO_SERVER_PID, entropy_msg, 1000)
        .expect("crypto_server reseed timeout");

    log::info!("Phoenix wake: crypto_server reseeded (epoch=0x{:016X})", epoch_id);

    // ─── 5. Séquence de redémarrage servers (CORR-35) ─────────────────
    if let Err(e) = restore_sequence::execute_restore_sequence() {
        log::error!("Phoenix wake: restore sequence failed {:?}", e);
        // Phase 8 : continuer malgré l'erreur (non bloquant)
    }
}
```

---

## 4. Erreurs Silencieuses ExoPhoenix

| Erreur | Symptôme | Détection |
|--------|----------|-----------|
| Pas de timeout spin-wait | Deadlock si Kernel B crash | CORR-33 : timeout 100ms |
| Pas de reseed crypto | Nonce reuse = chiffrement cassé | CORR-12 : test nonce ≠ après restore |
| VFS sans sync_fs avant ACK | Fichiers corrompus post-restore | CORR-13 : hash avant/après |
| DMA actif pendant snapshot | Snapshot RAM corrompu | CORR-14 : bus master disable |
| FPU non sauvegardée avant gel | Calculs corrompus post-restore | CORR-15 : test AVX avant/après |
| masked_since non reset | Faux positifs watchdog post-restore | test délai watchdog |
| Ordre restore mauvais | vfs_server démarre avant crypto → deadlock | CORR-35 : logs séquence |

---

*ExoOS — Guide d'Implémentation GI-05 : ExoPhoenix — Mars 2026*

---

# ExoOS — Guide d'Implémentation GI-06
## Servers Ring 1, IPC, Capabilities

**Prérequis** : GI-01..05  
**Produit** : `servers/*/`, `libs/exo-ipc/`

---

## 1. Template Server Ring 1

```rust
// servers/MON_SERVER/src/main.rs
//
// RÈGLES RING 1 ABSOLUES :
//
// PHX-02 : #![no_std] + panic = "abort" dans Cargo.toml
// CAP-01 : verify_cap_token() EN PREMIÈRE INSTRUCTION
// SRV-02 : AUCUN import blake3/chacha20poly1305
// IPC-02 : Tous les types protocol.rs : Sized, FixedString<N>, pas de Vec
// CORR-36/49 : panic handler via feature "panic_handler"

#![no_std]
#![no_main]

extern crate exo_ipc; // Fournit le panic handler via feature

use exo_types::{CapToken, CapabilityType};
use exo_types::ipc_msg::IpcMessage;

#[no_mangle]
pub extern "C" fn _start(boot_info_virt: usize) -> ! {
    // ─── CAP-01 : verify_cap_token EN PREMIER ────────────────────────
    // Si invalide → panic() → kernel envoie ChildDied à init_server (SRV-01)
    let boot_info = unsafe { &*(boot_info_virt as *const BootInfo) };
    exo_types::cap::verify_cap_token(&boot_info.my_server_cap, CapabilityType::MonServer);

    // ─── Validation BootInfo (CORR-38) ───────────────────────────────
    assert!(boot_info.validate(), "BootInfo invalide");

    // ─── Initialisation ──────────────────────────────────────────────
    init();

    // ─── Boucle principale IPC ────────────────────────────────────────
    loop {
        let msg = exo_ipc::receive::ipc_receive();
        handle_message(msg);
    }
}

fn handle_message(msg: IpcMessage) {
    // reply_nonce : copier dans la réponse pour validation côté client (CORR-17)
    match msg.msg_type {
        MSG_TYPE_REQUEST => handle_request(&msg),
        MSG_TYPE_PREPARE_ISOLATION => handle_prepare_isolation(&msg),
        _ => send_error_reply(&msg, ExoError::UnknownMessage),
    }
}

fn handle_prepare_isolation(req: &IpcMessage) {
    // PHX-01 : Sauvegarder état + répondre PrepareIsolationAck
    let ack = isolation::prepare_isolation();
    let reply = encode_isolation_ack(ack, req.reply_nonce);
    let _ = exo_ipc::send::ipc_send(req.sender_pid, reply);
}

// Cargo.toml
// [profile.dev]
// panic = "abort"
// [profile.release]
// panic = "abort"
// [dependencies]
// exo-ipc = { path = "../../libs/exo-ipc", features = ["panic_handler"] }
```

---

## 2. Ordre de Démarrage — Dépendances Clés

```
Étape 1 : libs/exo-types     → aucune dépendance
Étape 2 : libs/exo-ipc       → exo-types
Étape 3 : ipc_broker (PID 2) → rien (kernel l'assigne)
Étape 4 : memory_server      → ipc_broker
Étape 5 : init_server (PID 1)→ ipc_broker + memory_server + BootInfo virtuel
Étape 6 : vfs_server (PID 3) → init_server + ExoFS kernel monté
Étape 7 : crypto_server(PID4)→ vfs_server (besoin ExoFS pour stockage clés)
Étape 8 : device_server      → ipc_broker + memory_server (AVANT drivers)
Étape 9 : virtio-block       → device_server
Étape 10: virtio-net/console → device_server
Étape 11: network_server     → virtio-net
Étape 12: scheduler_server   → init_server
Étape 13: exo_shield         → Phase 3 Phoenix stable UNIQUEMENT

ERREUR SILENCIEUSE : crypto_server démarré APRÈS vfs_server
→ vfs_server essaie de calculer des hash au montage ExoFS
→ Deadlock : vfs attend crypto, crypto n'est pas encore là
→ CORR-35 : crypto_server TOUJOURS avant vfs_server
```

---

## 3. SpscRing — Implémentation IPC-01

```rust
// libs/exo-ipc/src/ring.rs
//
// IPC-01 : #[repr(C, align(64))] sur head ET tail (pas sur la struct entière)
//
// ❌ ERREUR COURANTE : repr(C, align(64)) sur la struct entière
//    → head et tail sont dans la MÊME cache line si petite struct
//    → False sharing : écrire head invalide la cache line de tail
//    → Performance catastrophique en SMP
//
// ✅ CORRECT : head ET tail dans des cache lines séparées

#[repr(C)]
pub struct SpscRing<T: Copy, const N: usize> {
    /// Producer side — doit être dans sa propre cache line
    #[repr(align(64))]
    head: AtomicUsize,

    /// Consumer side — doit être dans sa propre cache line
    #[repr(align(64))]
    tail: AtomicUsize,

    /// Buffer circulaire
    buffer: [UnsafeCell<T>; N],
}

unsafe impl<T: Copy + Send, const N: usize> Sync for SpscRing<T, N> {}
unsafe impl<T: Copy + Send, const N: usize> Send for SpscRing<T, N> {}

impl<T: Copy, const N: usize> SpscRing<T, N> {
    pub const fn new() -> Self {
        // N doit être une puissance de 2 pour le masquage
        assert!(N.is_power_of_two(), "SpscRing: N doit être une puissance de 2");
        SpscRing {
            head:   AtomicUsize::new(0),
            tail:   AtomicUsize::new(0),
            buffer: unsafe { core::mem::zeroed() },
        }
    }

    /// Push (côté producteur — un seul thread).
    pub fn push(&self, item: T) -> bool {
        let h = self.head.load(Ordering::Relaxed);
        let next_h = (h + 1) & (N - 1);
        // Vérifier si plein
        if next_h == self.tail.load(Ordering::Acquire) {
            return false; // Ring plein
        }
        unsafe { *self.buffer[h].get() = item; }
        self.head.store(next_h, Ordering::Release);
        true
    }

    /// Pop (côté consommateur — un seul thread).
    pub fn pop(&self) -> Option<T> {
        let t = self.tail.load(Ordering::Relaxed);
        if t == self.head.load(Ordering::Acquire) {
            return None; // Ring vide
        }
        let item = unsafe { *self.buffer[t].get() };
        self.tail.store((t + 1) & (N - 1), Ordering::Release);
        Some(item)
    }
}
```

---

## 4. Protocol.rs — Règles IPC-02 Strictes

```rust
// servers/vfs_server/src/protocol.rs
//
// RÈGLES IPC-02 :
//   ✅ Types Sized uniquement
//   ✅ FixedString<N> pour les chaînes
//   ❌ INTERDIT : &str, String, Vec<T>, Box<T>
//   ❌ INTERDIT : payload > 48B (CORR-31)
//
// POUR LES CHEMINS LONGS (PathBuf peut être > 48B) :
//   Utiliser un SHM handle dans le payload, données dans SHM

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpenRequest {
    pub path:        PathBuf,     // FixedString<512> — dans SHM si > 48B
    pub flags:       u32,
    pub _pad:        [u8; 12],
}

// Si PathBuf doit traverser IPC inline, utiliser SHM :
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpenRequestShm {
    /// Handle vers la page SHM contenant le path complet
    pub path_shm:    ObjectId,    // 24B — handle SHM
    pub path_len:    u32,         // Longueur du path dans le SHM
    pub flags:       u32,
    pub _pad:        [u8; 16],
}
const _: () = assert!(core::mem::size_of::<OpenRequestShm>() <= 48);

// Réponse avec FD
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpenResponse {
    pub fd:          u32,
    pub error:       u32,     // 0 = succès, sinon code erreur
    pub _pad:        [u8; 40],
}
const _: () = assert!(core::mem::size_of::<OpenResponse>() <= 48);
```

---

## 5. Erreurs Silencieuses Servers Ring 1

| Erreur | Symptôme | Détection |
|--------|----------|-----------|
| verify_cap_token() manquant | Accès sans auth silencieux | CI grep CAP-01 |
| blake3 import (SRV-02) | CI failure | CI grep check |
| panic=unwrap (PHX-02) | Crash sans notification | CI grep unwrap |
| payload > 48B IPC | UB/corruption | CORR-31 : CI scan |
| &str dans protocol.rs | Compile en std, crash no_std | CI grep IPC-02 |
| crypto_server après vfs_server | Deadlock boot | Vérifier ordre startup |
| SpscRing N non puissance de 2 | Corruption indexing | assert! dans new() |
| head/tail même cache line | Perf catastrophique SMP | Mesurer débit IPC |
| sender_pid:0 dans reply | init_server confus | CORR-49 : kernel SRV-01 |

---

## 6. Tests de Validation Phase 5

```bash
# Boot complet tous servers (QEMU)
qemu-system-x86_64 -m 1G -smp 4 -kernel kernel.elf \
  -drive file=exoos.img,format=raw -serial stdio
# ATTENDU : tous les 13 serveurs démarrés dans l'ordre

# Test IPC basique
# PID 3 (vfs_server) : open(/test.txt)
# ATTENDU : fd valide retourné

# Test verify_cap_token constant-time
# Mesurer le temps de vérification avec valeur valide vs invalide
# ATTENDU : temps identiques (± bruit) = pas de timing channel

# Test crypto reseed post-Phoenix
# Gel → restore → vérifier que les nonces sont différents
# ATTENDU : epoch_id différent, nonces différents post-restore
```

---

*ExoOS — Guides d'Implémentation GI-04, GI-05, GI-06 — Mars 2026*
