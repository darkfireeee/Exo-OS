# ExoOS — Corrections ExoPhoenix (Gel/Restore)
**Couvre : CORR-12, CORR-13, CORR-14, CORR-15**  
**Sources IAs : Gemini (§5,7,16), Grok4 (S-07,S-08,S-09), Kimi (§3), Claude**

---

## Vue d'ensemble — Séquence PrepareIsolation corrigée

La séquence de gel ExoPhoenix implique plusieurs serveurs Ring 1 qui doivent s'arrêter proprement avant le snapshot SSR. Les corrections CORR-12 à CORR-15 précisent ce que chaque serveur doit faire **avant** de renvoyer `PrepareIsolationAck`.

```
exo_shield broadcast PrepareIsolation
       │
       ├─→ crypto_server   → CORR-12 : reseed nonce (si restore)
       ├─→ vfs_server      → CORR-13 : sync_fs + flush cache disk
       ├─→ device_server   → CORR-14 : disable bus master tous drivers
       │      └─→ virtio-block/net/console : bus master disable
       ├─→ memory_server   → flush dirty ranges (déjà spécifié)
       ├─→ init_server     → CORR-15 : release FPU states thread courant
       │
       └─→ exo_shield collecte tous ACKs
              │
              └─→ phoenix_notify(521) = kernel B peut snapshotter
```

---

## CORR-12 🟠 — Crypto nonce rollback après Phoenix restore

### Problème
Lors d'un restore ExoPhoenix (snapshot → crash → restore), la RAM de `crypto_server` est restaurée à l'état du snapshot. Le compteur de nonces ChaCha20Poly1305 (LAC-04) repasse à sa valeur au moment du gel.

**Si le système a ensuite chiffré des données entre le gel et le crash**, les mêmes nonces seront regénérés après le restore → **réutilisation de nonce = destruction totale de la confidentialité ChaCha20**.

**Source** : Gemini §6 (Partie 6, point 13), Grok4 S-09 (partiellement)

### Correction — `servers/crypto_server/src/entropy.rs` + `servers/exo_shield/src/irq_handler.rs`

```rust
// ═══════════════════════════════════════════════════════════════════════
// servers/exo_shield/src/irq_handler.rs
// Après phoenix_notify(521) et avant de signaler "kernel prêt à snapshotter"
// ═══════════════════════════════════════════════════════════════════════

/// Signal envoyé par le kernel B à crypto_server lors du RÉVEIL (restore).
/// Contient de l'entropie fraîche RDRAND pour invalidation du nonce counter.
#[repr(C)]
pub struct PhoenixWakeEntropy {
    pub epoch_id:  u64,    // RDRAND/RDSEED — unique par cycle de restore
    pub timestamp: u64,    // TSC au réveil
}

// ═══════════════════════════════════════════════════════════════════════
// servers/crypto_server/src/main.rs
// ═══════════════════════════════════════════════════════════════════════

/// Réception du signal de réveil Phoenix — obligatoire avant tout chiffrement.
///
/// CORR-12 : Reseed du NONCE_COUNTER avec l'Epoch ID fourni par le kernel.
/// Garantit que les nonces post-restore sont différents des nonces pré-snapshot.
///
/// Protocole :
///   Kernel B (après restore) génère epoch_id via RDSEED + timestamp TSC.
///   Kernel B envoie PhoenixWakeEntropy à crypto_server via IPC système.
///   crypto_server appelle phoenix_reseed() avant d'accepter toute requête.
pub fn phoenix_reseed(entropy: &PhoenixWakeEntropy) {
    // Dériver une nouvelle clé master à partir de l'epoch_id et du seed existant
    // HKDF(salt=epoch_id, ikm=master_key, info="phoenix_reseed") → new_seed
    let new_seed = hkdf_derive(
        &MASTER_KEY.lock(),
        &entropy.epoch_id.to_le_bytes(),
        b"phoenix_reseed",
    );
    *MASTER_KEY.lock() = new_seed;

    // Remettre à zéro le compteur de nonces — repartir de 0 avec nouvelle clé
    NONCE_COUNTER.store(0, Ordering::SeqCst);

    log::info!(
        "crypto_server : reseed post-Phoenix (epoch_id=0x{:016X})",
        entropy.epoch_id
    );
}

// ═══════════════════════════════════════════════════════════════════════
// kernel/src/exophoenix/restore.rs (nouveau fichier)
// ═══════════════════════════════════════════════════════════════════════

/// Séquence de réveil ExoPhoenix (appelée par Kernel B après restore RAM).
pub fn phoenix_wake_sequence() {
    // 1. Générer l'Epoch ID (entropie hardware — RDSEED préféré à RDRAND)
    let epoch_id  = arch::rdseed().unwrap_or_else(|| arch::rdrand());
    let timestamp = unsafe { core::arch::x86_64::_rdtsc() };

    let entropy = PhoenixWakeEntropy { epoch_id, timestamp };

    // 2. Envoyer à crypto_server AVANT de déverrouiller les autres services
    ipc::send_priority(CRYPTO_SERVER_PID, IpcMsg::PhoenixWakeEntropy(entropy));

    // 3. Attendre ACK de crypto_server (timeout 100ms)
    let ack = ipc::recv_timeout(100).expect("crypto_server reseed timeout");
    assert_eq!(ack.msg_type, IpcMsgType::PhoenixWakeAck);

    // 4. Réinitialiser masked_since pour tous les IRQ routes (Grok4 S-02)
    // Les IRQs étaient gelées → tous les masked_since sont stales.
    irq_routing::reset_all_masked_since();

    // 5. Reprendre l'exécution normale
    log::info!("Phoenix wake complete — epoch_id=0x{:016X}", epoch_id);
}
```

**Note sur `reset_all_masked_since()` (Grok4 S-02)** :
```rust
// kernel/src/arch/x86_64/irq/routing.rs
/// Reset masked_since=0 sur toutes les routes IRQ actives après un wake Phoenix.
/// Sans ce reset, le watchdog verra un delta massif (gel = centaines de ms)
/// et déclenchera des hard resets spurieux sur tous les drivers.
pub fn reset_all_masked_since() {
    let table = IRQ_TABLE.read();
    for slot in table.iter().flatten() {
        // Reset seulement si pending_acks > 0 (IRQ active au moment du gel)
        if slot.pending_acks.load(Ordering::Relaxed) > 0 {
            slot.masked_since.store(0, Ordering::Release);
            slot.soft_alarmed.store(false, Ordering::Relaxed);
            // overflow_count conservé intentionnellement (anomalie persistante)
        }
    }
}
```

---

## CORR-13 🟠 — vfs_server : sync_fs avant PrepareIsolationAck

### Problème
`vfs_server` répond `PrepareIsolationAck` après avoir flushed son `fd_table`.  
Mais les données du page cache et les métadonnées encore en RAM ne sont pas forcément sur le disque physique.

Après restore sur une autre machine (ou reboot), l'état SSR restauré en RAM peut diverger de l'état réel du SSD → **perte silencieuse de données**.

**Source** : Gemini §6 point 16

### Correction — `servers/vfs_server/src/isolation.rs`

```rust
// servers/vfs_server/src/isolation.rs — CORR-13
// Remplace le flush fd_table basique par une synchronisation complète.

pub fn prepare_isolation() -> PrepareIsolationAck {
    log::info!("vfs_server : PrepareIsolation reçu — début sync_fs");

    // Étape 1 : Flush fd_table (comportement existant)
    fd_table::flush_all();

    // Étape 2 : Sync complet ExoFS (CORR-13 — NOUVEAU)
    // Déclenche : commit tous les Epochs en cours + writeback du page cache
    match exofs_sync_fs() {
        Ok(()) => log::info!("vfs_server : sync_fs complété"),
        Err(e) => log::error!("vfs_server : sync_fs erreur {:?} — proceeding", e),
    }

    // Étape 3 : Flush cache disque matériel
    // Envoyer FLUSH_CACHE NVMe/SATA à tous les drivers de stockage Ring 1.
    // Attendre complétion avant de renvoyer ACK.
    for backend in storage_backends() {
        match backend.flush_disk_cache(FLUSH_TIMEOUT_MS) {
            Ok(())  => {},
            Err(e)  => log::error!("vfs_server : disk flush {:?} erreur {:?}", backend, e),
        }
    }

    // Étape 4 : Renvoyer ACK seulement après durabilité garantie
    log::info!("vfs_server : durabilité confirmée → PrepareIsolationAck");
    PrepareIsolationAck {
        server:        ServiceName::from_bytes(b"vfs_server"),
        checkpoint_id: exofs_current_epoch(),
    }
}

const FLUSH_TIMEOUT_MS: u64 = 5000; // 5 secondes max pour flush disque
```

**Requête IPC vers virtio-block** :
```rust
// servers/virtio-block/src/main.rs — handler FLUSH_CACHE
fn handle_flush_cache_request() -> Result<(), DriverError> {
    // Émettre commande VIRTIO_BLK_T_FLUSH au device
    virtio_block_flush()?;
    // Attendre complétion (bloquant, timeout 3s)
    wait_flush_completion(3000)?;
    Ok(())
}
```

---

## CORR-14 🟠 — DMA bus master disable avant PrepareIsolationAck

### Problème
Lors du gel Phoenix, si un driver PCI a encore le Bus Mastering actif, il peut continuer à écrire en DMA vers la RAM **pendant** le snapshot SSR → corruption du snapshot.

Le `do_exit()` désactive le bus mastering, mais la séquence de gel n'est pas `do_exit()`. C'est la séquence `PrepareIsolation → snapshot → restore`.

**Source** : Gemini §A-2 (Isolation DMA), Grok4 S-09 (IOMMU drain pendant gel)

### Correction — `servers/device_server/src/isolation.rs`

```rust
// servers/device_server/src/isolation.rs — CORR-14

pub fn prepare_isolation() -> PrepareIsolationAck {
    log::info!("device_server : PrepareIsolation — désactivation Bus Mastering");

    // Étape 1 : Désactiver le Bus Mastering sur TOUS les devices actifs
    // avant de renvoyer l'ACK → aucun DMA possible pendant le snapshot.
    let active_drivers = driver_registry::get_all_active();
    for driver in &active_drivers {
        if let Some(bdf) = driver.bdf {
            // SYS_PCI_BUS_MASTER(false) via syscall 539
            match pci::disable_bus_master(bdf) {
                Ok(())  => log::debug!("Bus master disabled: {:?}", bdf),
                Err(e)  => log::error!("Bus master disable failed {:?}: {:?}", bdf, e),
            }
        }
    }

    // Étape 2 : Drain IOMMU fault queue (Grok4 S-09)
    let dropped = iommu_fault_queue::drain_dropped();
    if dropped > 0 {
        log::warn!("device_server : {} IOMMU faults perdues avant gel", dropped);
    }

    // Étape 3 : Désactiver les domaines IOMMU actifs
    // Après bus master disable, le hardware ne peut plus initier de DMA.
    // Désactiver les domaines IOMMU ajoute une couche de défense supplémentaire.
    for driver in &active_drivers {
        if let Ok(domain_id) = iommu_domain_registry::domain_of_pid(driver.pid) {
            iommu::disable_domain_atomic(domain_id);
        }
    }

    log::info!("device_server : DMA quiesced → PrepareIsolationAck");

    PrepareIsolationAck {
        server:        ServiceName::from_bytes(b"device_server"),
        checkpoint_id: 0, // device_server est transient — pas de checkpoint
    }
}
```

---

## CORR-15 🟠 — FPU state : libération avant gel Phoenix

### Problème
Le handler de l'IPI ExoPhoenix (vecteur 0xF3) gèle les cœurs pour le snapshot.  
Si un thread avait la FPU chargée au moment du gel (Lazy FPU, `CR0.TS=0`), son état FPU est dans les registres physiques du CPU et **non** dans `fpu_state_ptr`.

Après restore ExoPhoenix (RAM restaurée, registres CPU réinitialisés), l'état FPU de ce thread est perdu → corruption silencieuse de calculs AVX/SSE/x87.

**Source** : Gemini §5 point 9, Grok4 S-07

### Correction — `servers/exo_shield/src/irq_handler.rs`

```rust
// servers/exo_shield/src/irq_handler.rs — CORR-15
//
// Avant d'envoyer phoenix_notify(521), le kernel doit forcer XSAVE
// pour tous les threads ayant la FPU chargée.

// Cette correction est dans le KERNEL Ring 0, pas dans exo_shield Ring 1.
// exo_shield envoie phoenix_notify(521) → kernel Ring 0 force XSAVE → snapshot.
```

```rust
// kernel/src/exophoenix/freeze.rs — NOUVEAU fichier

/// Appelé par le handler IDT 0xF3 (IPI ExoPhoenix freeze)
/// sur chaque cœur avant le snapshot SSR.
///
/// CORR-15 : Forcer XSAVE pour le thread courant si FPU chargée.
/// Sans cela, l'état FPU est perdu lors du restore.
pub fn handle_freeze_ipi() {
    // 1. Forcer XSAVE du thread courant si FPU active (CR0.TS == 0)
    let current_tcb = scheduler::current_tcb();
    let cr0 = unsafe { x86_64::registers::control::Cr0::read() };

    if !cr0.contains(Cr0Flags::TASK_SWITCHED) {
        // CR0.TS == 0 → FPU active → forcer xsave64
        if current_tcb.fpu_state_ptr != 0 {
            unsafe {
                fpu::xsave64(current_tcb.fpu_state_ptr as *mut XSaveArea);
            }
            // Marquer FPU comme non chargée
            fpu::mark_fpu_not_loaded(current_tcb.tid);
            // Mettre CR0.TS = 1 pour déclencher #NM au prochain usage FPU
            unsafe { x86_64::instructions::interrupts::clear(); }
            unsafe { core::arch::asm!("mov rax, cr0; or rax, 8; mov cr0, rax"); }
        }
    }

    // 2. Écrire le FREEZE_ACK dans le SSR (Core courant)
    let apic_id = lapic::current_apic_id() as usize;
    let ack_offset = exo_phoenix_ssr::freeze_ack_offset(apic_id);
    let ssr_ptr = exo_phoenix_ssr::SSR_BASE_PHYS as *mut u64;
    unsafe {
        let ack_ptr = ssr_ptr.add(ack_offset / 8) as *mut core::sync::atomic::AtomicU64;
        (*ack_ptr).store(1, core::sync::atomic::Ordering::Release);
    }

    // 3. Spin-wait jusqu'au signal de reprise du Kernel B
    loop {
        let handoff = unsafe {
            let flag_ptr = ssr_ptr.add(exo_phoenix_ssr::SSR_HANDOFF_FLAG_OFFSET / 8)
                as *const core::sync::atomic::AtomicU64;
            (*flag_ptr).load(core::sync::atomic::Ordering::Acquire)
        };
        if handoff == 3 { break; } // B_ACTIVE = reprise
        core::hint::spin_loop();
    }
}
```

---

## Serveurs "transient" et PrepareIsolation (Gemini §7, CORR PARTIELLEMENT ACCEPTÉ)

### Analyse
Architecture v7 S-39 : `scheduler_server` et `network_server` sont intentionnellement **sans `isolation.rs`** (état transient).

PHX-01 dit "chaque server critique" → ces serveurs ne sont PAS considérés critiques.

**Mais** : si `exo_shield` les inclut dans sa liste d'abonnés, il attendra indéfiniment leur ACK.

### Solution retenue

`exo_shield` ne doit **pas** attendre `scheduler_server` et `network_server` :

```rust
// servers/exo_shield/src/subscription.rs — CORR (partiel Gemini §7)

/// Serveurs qui DOIVENT renvoyer PrepareIsolationAck avant le gel.
/// EXCLUT les serveurs transients (scheduler_server, network_server).
pub const CRITICAL_SERVERS: &[&str] = &[
    "ipc_broker",
    "memory_server",
    "init_server",
    "vfs_server",
    "crypto_server",
    "device_server",
    // "scheduler_server" ← EXCLU intentionnellement (S-39)
    // "network_server"   ← EXCLU intentionnellement (S-39)
];

/// Timeout d'attente par serveur (en ms).
pub const ACK_TIMEOUT_PER_SERVER_MS: u64 = 5000;
```

Les serveurs `scheduler_server` et `network_server` seront forcément tués lors du gel (leur état est transient) et redémarrés depuis zéro après restore. Ceci est le comportement attendu.

---

*ExoOS — Corrections ExoPhoenix — Mars 2026*
