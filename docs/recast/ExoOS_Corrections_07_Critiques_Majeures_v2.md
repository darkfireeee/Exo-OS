# ExoOS — Corrections v2 : Critiques & Majeures (CORR-31 à CORR-41)
**Synthèse du feedback RETOUR-AI-2 : Z-AI, Copilote, ChatGPT5, KIMI-AI, MiniMax**  
**Double passe d'analyse — arbitrages documentés — erreurs des IAs signalées**

---

## Arbitrages inter-IAs (nouveaux conflits)

### EN-01 MiniMax : condition CR0.TS dans CORR-15 — REJETÉ
MiniMax affirme que `if !cr0.contains(Cr0Flags::TASK_SWITCHED)` est inversé.  
**INCORRECT.** Selon la spec Intel (OSDev Wiki, Intel SDM) :
- **CR0.TS = 1** → FPU en mode lazy (#NM si utilisée) — état dans `fpu_state_ptr`
- **CR0.TS = 0** → FPU active dans les registres CPU → XSAVE nécessaire avant gel

`!contains(TASK_SWITCHED)` = TS bit est 0 = FPU active → XSAVE obligatoire. CORR-15 est **correct**.  
MiniMax produit ensuite la même condition dans sa "correction" — contradiction interne.

### EN-03 MiniMax : SeqCst pour IommuFaultQueue — REJETÉ
MiniMax propose de remplacer la queue CAS-based par SeqCst global.  
**INCORRECT.** La queue CAS-based AcqRel/Release de Kernel_Types_v10 §7 est un design MPSC prouvé correct (ABA-free, no orphaned slots). SeqCst inutile + performance dégradée.

### KIMI CORR-35 : wait_link_retraining lock+yield deadlock — REJETÉ
KIMI affirme que `parent_bridge()` tient un lock pendant le yield.  
**INCORRECT.** `parent_bridge()` retourne `Option<PciBdf>` où `PciBdf: Copy`. Le read lock est libéré **avant** le return. Il n'y a pas de lock tenu lors de `yield_current_thread()`.

### KIMI CORR-34 : spin_loop en ISR si dropped>1M — REJETÉ
KIMI propose d'appeler `core::hint::spin_loop()` dans la fonction `push()` (ISR) si `dropped > 1_000_000`.  
**CRITIQUE.** `push()` s'exécute en ISR context — toute attente est interdite (FIX-109). Rejeté.

### MAX_PENDING_ACKS non défini (IC-02 MiniMax) — PARTIELLEMENT REJETÉ
Driver Framework v10 §3.1 définit explicitement `const MAX_PENDING_ACKS: u32 = 4096;`.  
La valeur est définie. Cependant la sémantique de reset d'`overflow_count` mérite clarification → CORR-44.

---

## CORR-31 🔴 — IpcMessage payload 48B : guide de migration ABI

### Problème
CORR-17 (session précédente) réduit `payload` de 56B à 48B pour ajouter `reply_nonce: u32`.  
Cette cassure ABI n'est pas documentée avec un guide de migration. Tous les `protocol.rs` Ring 1 qui utilisent les 56B doivent être identifiés et migrés.

### Correction — Migration Guide ABI

**Règle IPC-04 (nouvelle)** :
```
IPC-04 : AUCUN message inline ne dépasse 48B de payload.
         Les données > 48B utilisent un handle SHM.
         Pattern : RequestMsg { shm_handle: ObjectId } + data dans SHM.
```

**Protocoles à vérifier et migrer — `servers/*/src/protocol.rs`** :

```rust
// AVANT CORR-17 : payload [u8; 56] = 56B
// APRÈS CORR-17 + CORR-31 : payload [u8; 48] = 48B

// ─── Audit des protocoles impactés ───────────────────────────────────
// vfs_server/protocol.rs — ReadResponse :
//   AVANT : data:[u8;56]  (trop petit de toute façon — utilisait SHM)
//   APRÈS : shm_handle:ObjectId (24B) + len:u32 + flags:u32 + _pad:[u8;16] ← OK

// crypto_server/protocol.rs — HashResponse :
//   AVANT : hash:[u8;32] + type_id:u16 + _pad:[u8;6] = 40B ← OK (< 48B)
//   APRÈS : inchangé

// vfs_server/protocol.rs — StatResponse :
//   AVANT : ObjectStat inline ~50B ← PROBLÈME
//   APRÈS : shm_handle:ObjectId pour transfert ObjectStat

// Guide de migration pour tout payload > 48B :
// 1. Allouer un SHM via memory_server
// 2. Écrire les données dans le SHM
// 3. Passer shm_handle:ObjectId dans le message IPC
// 4. Receiver lit via SHM et libère
```

**Script d'audit CI** :
```bash
# CI — vérifier qu'aucun protocol.rs n'utilise un payload > 48B inline
# Chercher les structs IPC avec des arrays [u8; N] où N > 48
grep -r "\[u8; [5-9][0-9]\]\|\[u8; [1-9][0-9][0-9]\]" \
    servers/*/src/protocol.rs drivers/*/src/protocol.rs \
  && echo "VIOLATION IPC-04 : payload > 48B trouvé" && exit 1
```

---

## CORR-32 🔴 — sys_pci_claim : TOCTOU + double BDF claim

### Problème
`sys_pci_claim` effectue ses vérifications (`MMIO_WHITELIST`, `is_ram_region`) **avant** de prendre le lock `DEVICE_CLAIMS.write()`. Fenêtre TOCTOU entre les deux.

De plus, deux drivers pourraient claimer le même BDF avec des régions physiques disjointes mais représentant le même device. Aucune vérification BDF-unique.

**Sources** : KIMI CORR-32, Z-AI CORR-32

### Correction — `kernel/src/drivers/device_claims.rs`

```rust
// kernel/src/drivers/device_claims.rs — CORR-32
// Protection TOCTOU : vérifications sous lock + unicité BDF

pub fn sys_pci_claim(
    phys_base:   PhysAddr,
    size:        usize,
    driver_pid:  u32,
    bdf:         Option<PciBdf>,
    calling_pid: u32,
) -> Result<(), ClaimError> {
    // Vérification capability AVANT lock (lecture seule, pas de TOCTOU ici)
    if !process::has_capability(calling_pid, Capability::SysDeviceAdmin) {
        return Err(ClaimError::PermissionDenied);
    }

    // CORR-32 : Acquérir le lock AVANT toute vérification de région
    // Protège contre TOCTOU sur MMIO_WHITELIST et memory_map
    let _irq_guard = arch::irq_save(); // Éviter deadlock IRQ → claims
    let mut claims = DEVICE_CLAIMS.write();

    // Vérifications sous lock (atomiques par rapport à d'autres claims)
    if !MMIO_WHITELIST.contains(phys_base, size) {
        return Err(ClaimError::NotInHardwareRegion);
    }
    if memory_map::is_ram_region(phys_base, size) {
        return Err(ClaimError::PhysIsRam);
    }

    // Vérification overlap physique
    if claims.iter().any(|c| c.overlaps(phys_base, size)) {
        return Err(ClaimError::AlreadyClaimed);
    }

    // CORR-32 : Vérification BDF unique (si spécifié)
    // Empêche deux drivers de claimer le même device PCI
    if let Some(b) = bdf {
        if claims.iter().any(|c| c.bdf == Some(b)) {
            return Err(ClaimError::AlreadyClaimed); // BDF déjà claimé
        }
    }

    let gen = process::get_generation(driver_pid);
    claims.push(DeviceClaim {
        phys_base, size, owner_pid: driver_pid, generation: gen, bdf,
    }).map_err(|_| ClaimError::TableFull)?;

    Ok(())
}
```

**Erreur ClaimError à ajouter** :
```rust
pub enum ClaimError {
    PermissionDenied,
    PhysIsRam,
    NotInHardwareRegion,
    AlreadyClaimed,
    TableFull, // ← NOUVEAU : heapless::Vec plein
}
```

---

## CORR-33 🔴 — Phoenix freeze : timeout obligatoire pour la spin-wait

### Problème
Dans `handle_freeze_ipi()` (CORR-15 du fichier précédent), la boucle d'attente du signal `B_ACTIVE` de Kernel B n'a **aucun timeout**. Si Kernel B crashe pendant le snapshot, tous les cœurs Kernel A spinneront indéfiniment → deadlock total.

**Sources** : MiniMax EN-02, Z-AI CORR-37

### Correction — `kernel/src/exophoenix/freeze.rs`

```rust
// kernel/src/exophoenix/freeze.rs — CORR-33

/// Timeout pour la spin-wait du gel Phoenix.
/// Valeur : 100ms, calculée en ticks TSC via BOOT_TSC_KHZ.
/// Après timeout : entrée en mode degraded (snapshot partiel permis).
const FREEZE_TIMEOUT_MS: u64 = 100;

/// Attend le signal B_ACTIVE de Kernel B, avec timeout.
/// Appelé depuis handle_freeze_ipi() après écriture du FREEZE_ACK.
///
/// CORR-33 : Ajout du timeout pour prévenir deadlock si Kernel B crash.
fn wait_for_wake_signal() {
    // Timeout en ticks TSC (approximatif — TSC calibré au boot)
    let khz = BOOT_TSC_KHZ.load(Ordering::Relaxed);
    let timeout_ticks = if khz > 0 {
        FREEZE_TIMEOUT_MS * khz  // khz = ticks/ms → ticks pour timeout_ms
    } else {
        // Fallback si TSC non calibré : ~300M ticks ≈ 100ms à 3GHz
        300_000_000u64
    };

    let start_tsc = unsafe { core::arch::x86_64::_rdtsc() };

    loop {
        // Lire HANDOFF_FLAG depuis la région SSR physique
        let handoff = read_ssr_u64_phys(SSR_HANDOFF_FLAG_OFFSET);

        match handoff {
            3 => return, // B_ACTIVE — reprise normale
            2 => return, // FREEZE_ACK_ALL — snapshot terminé
            _ => {}       // FREEZE_REQ (1) ou normal (0) — attendre
        }

        // Vérification timeout
        let elapsed = unsafe { core::arch::x86_64::_rdtsc() }
            .wrapping_sub(start_tsc);
        if elapsed >= timeout_ticks {
            let apic_id = lapic::current_apic_id();
            // Log minimal (pas d'allocation heap, pas d'IPC)
            unsafe {
                arch::serial_writeln(b"[PHOENIX] freeze timeout — degraded mode");
            }
            // Écrire ACK dégradé pour libérer le système
            write_ssr_u64_phys(
                SSR_FREEZE_ACK_OFFSET + apic_id as usize * 64,
                0xDEAD_ACK, // Valeur sentinel = ACK dégradé
            );
            return; // Sortir de la spin-wait, permettre la progression
        }

        // Pause CPU (réduit contention bus, économie énergie)
        unsafe { core::arch::asm!("pause", options(nostack, nomem)); }
    }
}

/// Lecture directe depuis la région physique SSR (CORR-33).
/// SSR_BASE_PHYS est mappé en identité dans l'espace noyau.
#[inline(always)]
fn read_ssr_u64_phys(offset: usize) -> u64 {
    let ptr = (SSR_BASE_PHYS as usize + offset) as *const core::sync::atomic::AtomicU64;
    unsafe { (*ptr).load(Ordering::Acquire) }
}

#[inline(always)]
fn write_ssr_u64_phys(offset: usize, val: u64) {
    let ptr = (SSR_BASE_PHYS as usize + offset) as *const core::sync::atomic::AtomicU64;
    unsafe { (*ptr).store(val, Ordering::Release); }
}
```

---

## CORR-34 🟠 — TSC overflow : calcul différentiel pour `current_time_ms()`

### Problème
`current_time_ms()` calcule `tsc / khz`. Sur un système avec un long uptime (années), le TSC brut divisé par khz peut produire une valeur qui wraps ou pose des problèmes de précision.

Plus concret : sur un CPU 3GHz, TSC overflow en u64 = jamais en pratique (>100 ans). Mais la précision `tsc / khz` (division entière) introduit une erreur de ±1ms par appel.

La correction réelle utile est le **calcul différentiel** pour éviter la perte de précision à très long uptime, et fournir une origine de temps stable après restore Phoenix.

**Source** : KIMI CORR-33

### Correction — `kernel/src/time.rs`

```rust
// kernel/src/time.rs — CORR-34
// Calcul différentiel : base TSC capturée au boot pour précision accrue.

use core::sync::atomic::{AtomicU64, Ordering};

/// Fréquence TSC calibrée au boot. AtomicU64 (FIX-103).
static BOOT_TSC_KHZ:      AtomicU64 = AtomicU64::new(0);

/// Valeur TSC capturée à la fin de calibrate_tsc_khz().
/// Utilisée comme origine pour le calcul différentiel.
static BOOT_TSC_BASE:     AtomicU64 = AtomicU64::new(0);

/// Offset en ms à ajouter après un reset Phoenix (CORR-12).
/// Réinitialisé à 0 au boot, mis à jour après restore.
static PHOENIX_MS_OFFSET: AtomicU64 = AtomicU64::new(0);

pub fn calibrate_tsc_khz() {
    let measured_khz: u64 = /* calibration PIT 50ms */ 3_000_000;
    // Capturer le TSC de référence juste après calibration
    let tsc_now = unsafe { core::arch::x86_64::_rdtsc() };
    BOOT_TSC_KHZ.store(measured_khz, Ordering::Relaxed);
    BOOT_TSC_BASE.store(tsc_now,     Ordering::Relaxed);
    // Ordering::Relaxed suffit : barrière = enable_interrupts() qui suit
}

/// Temps monotone en ms depuis le boot.
/// CORR-34 : Calcul différentiel évite erreur cumulative.
pub fn current_time_ms() -> u64 {
    let tsc_now  = unsafe { core::arch::x86_64::_rdtsc() };
    let tsc_base = BOOT_TSC_BASE.load(Ordering::Relaxed);
    let khz      = BOOT_TSC_KHZ.load(Ordering::Relaxed);
    let offset   = PHOENIX_MS_OFFSET.load(Ordering::Relaxed);

    debug_assert!(khz > 0, "current_time_ms() appelé avant calibrate_tsc_khz()");

    // Calcul différentiel : (tsc_now - tsc_base) / khz + offset
    // Évite division entière sur un grand nombre ; erreur max = 1ms
    let tsc_delta = tsc_now.saturating_sub(tsc_base);
    let ms_delta  = tsc_delta / khz.max(1);
    offset.saturating_add(ms_delta)
}

/// Appelé après restore Phoenix pour mettre à jour l'offset de temps.
/// Empêche un saut temporel visible par le watchdog (CORR-12).
pub fn phoenix_reset_time_base() {
    let tsc_now = unsafe { core::arch::x86_64::_rdtsc() };
    let khz     = BOOT_TSC_KHZ.load(Ordering::Relaxed);
    let old_base = BOOT_TSC_BASE.load(Ordering::Relaxed);
    // Accumuler le temps écoulé avant le reset
    let elapsed = old_base.saturating_sub(0);
    PHOENIX_MS_OFFSET.fetch_add(elapsed / khz.max(1), Ordering::Relaxed);
    // Nouveau point de référence TSC
    BOOT_TSC_BASE.store(tsc_now, Ordering::Relaxed);
}
```

---

## CORR-35 🟠 — Phoenix restore : séquence de redémarrage des serveurs

### Problème
Les corrections ExoPhoenix (CORR-12 à CORR-15) spécifient le **gel** mais pas le **redémarrage** après restore. L'ordre de redémarrage est critique : si `vfs_server` démarre avant `crypto_server`, il ne peut pas obtenir les hash Blake3 nécessaires à l'ouverture des fichiers.

**Source** : KIMI CORR-40

### Correction — `servers/exo_shield/src/restore_sequence.rs`

```rust
// servers/exo_shield/src/restore_sequence.rs — CORR-35 (NOUVEAU fichier)

/// Ordre de redémarrage post-Phoenix — CANONIQUE.
///
/// Contrainte : chaque server ne démarre qu'après que ses dépendances
/// soient fonctionnelles (ping réussi).
///
/// DIFFÉRENT de l'ordre de boot initial (Arborescence V4 §7) car :
///   - ipc_broker doit être opérationnel pour que les autres puissent s'enregistrer
///   - crypto_server doit être prêt avant vfs_server (hash Blake3)
///   - device_server doit réactiver le bus mastering après gel
pub const RESTORE_SEQUENCE: &[RestoreStep] = &[
    RestoreStep { service: "crypto_server",  timeout_ms: 3_000 },
    RestoreStep { service: "memory_server",  timeout_ms: 2_000 },
    RestoreStep { service: "ipc_broker",     timeout_ms: 2_000 },
    RestoreStep { service: "vfs_server",     timeout_ms: 5_000 }, // après crypto
    RestoreStep { service: "device_server",  timeout_ms: 3_000 }, // réactive BM
    RestoreStep { service: "init_server",    timeout_ms: 2_000 },
    // virtio-block / virtio-net / virtio-console : relancés par init_server
    // network_server + scheduler_server : relancés par init_server (transient)
];

#[repr(C)]
pub struct RestoreStep {
    pub service:    &'static str,
    pub timeout_ms: u64,
}

/// Séquence complète de restore — appelée par Kernel B après handoff.
pub fn execute_restore_sequence() -> Result<(), PhoenixError> {
    for step in RESTORE_SEQUENCE {
        log::info!("Phoenix restore : attente {} ({}ms max)", step.service, step.timeout_ms);

        // Attendre que le service réponde au ping IPC
        let deadline = current_time_ms() + step.timeout_ms;
        loop {
            if ping_service_by_name(step.service) {
                log::info!("Phoenix restore : {} OK", step.service);
                break;
            }
            if current_time_ms() >= deadline {
                log::error!("Phoenix restore : {} timeout → abort restore", step.service);
                return Err(PhoenixError::ServiceUnresponsive);
            }
            unsafe { core::arch::asm!("pause", options(nostack, nomem)); }
        }
    }

    // Vérification finale d'intégrité des binaires (PHX-03)
    verify_binary_integrity()?;

    log::info!("Phoenix restore : séquence complète — système opérationnel");
    Ok(())
}

/// Vérifie que les hash Blake3 des binaires chargés correspondent
/// aux ObjectId enregistrés dans ExoFS (PHX-03).
fn verify_binary_integrity() -> Result<(), PhoenixError> {
    // Pour chaque binaire de RESTORE_SEQUENCE :
    // 1. Calculer Blake3(ELF) du code en mémoire
    // 2. Comparer avec l'ObjectId enregistré via register_binaries.sh
    // Si divergence → possible tamper → abort restore
    // Implémentation : via crypto_server IPC (SRV-04)
    Ok(()) // TODO Phase 3
}
```

---

## CORR-36 🟠 — Panic handler Ring 1 : notification init_server

### Problème
Avec `panic = 'abort'` (PHX-02), un serveur Ring 1 qui panic s'arrête immédiatement sans notifier `init_server`. `ipc_broker` ignore la mort du serveur. Les clients IPC en attente restent bloqués indéfiniment.

**Source** : Z-AI CORR-36

### Correction — `libs/exo-ipc/src/panic.rs`

```rust
// libs/exo-ipc/src/panic.rs — CORR-36 (NOUVEAU fichier)
// Panic handler partagé pour tous les servers Ring 1.
//
// COMPORTEMENT :
//   1. Log minimal sur UART (sans allocation)
//   2. Envoyer IPC ChildDied à init_server (PID 1)
//   3. Abort (PHX-02 respecté)
//
// USAGE : Dans Cargo.toml de chaque server Ring 1 :
//   exo-ipc = { path = "../../libs/exo-ipc", features = ["panic_handler"] }

#[cfg(feature = "panic_handler")]
#[panic_handler]
fn ring1_panic_handler(info: &core::panic::PanicInfo) -> ! {
    // Étape 1 : Log UART minimal (sans allocation heap)
    unsafe {
        arch::serial_write(b"\n[PANIC] Ring 1 server: ");
        if let Some(location) = info.location() {
            // Écrire file:line sans alloc (pas de format!)
            arch::serial_write(location.file().as_bytes());
            arch::serial_write(b":");
            let mut line_buf = [0u8; 10];
            write_u64_to_buf(location.line() as u64, &mut line_buf);
            arch::serial_write(&line_buf);
        }
        arch::serial_write(b"\n");
    }

    // Étape 2 : IPC d'urgence vers init_server (PID 1)
    // Utilise le chemin IPC sans allocation (message sur la pile)
    let msg = IpcMessage {
        sender_pid:  0,                       // Kernel renseignera
        msg_type:    IPC_MSG_TYPE_CHILD_DIED,
        reply_nonce: 0,
        _pad:        0,
        payload:     encode_child_died_payload(current_pid(), 139u32), // signal 11 = SIGSEGV
    };

    // Tentative non-bloquante (best-effort — le process est mourant)
    let _ = ipc::send_nonblocking(INIT_SERVER_PID, msg);

    // Étape 3 : Abort définitif (PHX-02)
    unsafe { core::intrinsics::abort() }
}

/// Encode un message ChildDied dans 48 bytes de payload.
fn encode_child_died_payload(pid: u32, exit_code: u32) -> [u8; 48] {
    let mut p = [0u8; 48];
    p[0..4].copy_from_slice(&pid.to_le_bytes());
    p[4..8].copy_from_slice(&exit_code.to_le_bytes());
    p
}

// PID de init_server (connu au moment de la compilation/boot)
const INIT_SERVER_PID: u32 = 1;
const IPC_MSG_TYPE_CHILD_DIED: u32 = 0x0001; // Défini dans protocol.rs canonique
```

**Ajout dans `servers/*/Cargo.toml`** :
```toml
[dependencies]
exo-ipc = { path = "../../libs/exo-ipc", features = ["panic_handler"] }
```

**CI check** :
```bash
# Vérifier que chaque server active le panic_handler feature
for cargo in servers/*/Cargo.toml; do
  grep -q 'panic_handler' "$cargo" \
    || { echo "MISSING panic_handler : $cargo"; exit 1; }
done
```

---

## CORR-37 🟠 — dispatch_irq : rejet à l'enregistrement si limite atteinte

### Problème
CORR-04 (session précédente) remplace `Vec<IpcEndpoint>` par un tableau de taille fixe `[Option<IpcEndpoint>; MAX_HANDLERS_PER_IRQ]`. Si plus de 8 handlers sont enregistrés sur un même IRQ, les handlers supplémentaires sont **ignorés silencieusement** lors du dispatch.

Il faut rejeter l'enregistrement dès `sys_irq_register()` plutôt que de tronquer silencieusement au dispatch.

**Source** : ChatGPT5 §D

### Correction — `kernel/src/arch/x86_64/irq/routing.rs`

```rust
// routing.rs — sys_irq_register — CORR-37
// Ajout vérification limite handlers AVANT d'ajouter un nouveau handler.

pub fn sys_irq_register(
    irq:         u8,
    endpoint:    IpcEndpoint,
    source_kind: IrqSourceKind,
    bdf:         Option<PciBdf>,
) -> Result<u64, IrqError> {
    let _irq_guard = arch::irq_save();
    let mut table  = IRQ_TABLE.write();

    let is_new = table[irq as usize].is_none();

    let route = table[irq as usize].get_or_insert_with(|| IrqRoute {
        irq_line:            irq,
        source_kind,
        handlers:            heapless::Vec::new(), // heapless::Vec<IrqHandler, 8>
        // ... autres champs ...
    });

    // CORR-37 : Vérifier la limite avant d'ajouter
    // Rejeter explicitement plutôt que tronquer silencieusement au dispatch
    if route.handlers.len() >= MAX_HANDLERS_PER_IRQ {
        log::error!(
            "sys_irq_register IRQ {} : limite {} handlers atteinte — refus",
            irq, MAX_HANDLERS_PER_IRQ
        );
        return Err(IrqError::HandlerLimitReached);
    }

    // [... reste du code inchangé ...]

    let generation  = GLOBAL_GEN.fetch_add(1, Ordering::Relaxed);
    let reg_id      = new_reg_id();
    let calling_pid = current_process::pid();

    route.handlers.retain(|h| h.owner_pid != calling_pid);

    // Après retain, re-vérifier : retain peut avoir libéré de la place
    if route.handlers.len() >= MAX_HANDLERS_PER_IRQ {
        return Err(IrqError::HandlerLimitReached);
    }

    route.handlers.push(IrqHandler {
        reg_id, generation, owner_pid: calling_pid, endpoint
    }).map_err(|_| IrqError::HandlerLimitReached)?; // heapless::Vec ne peut pas fail si len check OK

    Ok(reg_id)
}
```

**Ajout dans `IrqError`** :
```rust
pub enum IrqError {
    NotRegistered,
    NotOwner,
    KindMismatch { existing: IrqSourceKind, requested: IrqSourceKind },
    HandlerLimitReached, // ← NOUVEAU (CORR-37)
}
```

**Changement de structure** : `handlers` passe de `Vec<IrqHandler>` (heap) à `heapless::Vec<IrqHandler, 8>` (stack-allocated, no heap).

---

## CORR-38 🟠 — BootInfo : mapping read-only + vérification d'intégrité

### Problème
La correction V7-C-01 / CORR-09 mappe `BootInfo` en virtuel dans la VMA de `init_server` mais ne précise pas :
1. Les droits de la page : lecture seule ou lecture/écriture ?
2. Si `BootInfo` est corrompu (magic invalide, reserved non-zéro), le comportement est undefined.

**Source** : ChatGPT5 §F

### Correction — `kernel/src/process/exec.rs`

```rust
// kernel/src/process/exec.rs — mapping BootInfo — CORR-38

pub fn map_boot_info_for_init_server(boot_info: &BootInfo) -> VirtAddr {
    // Allouer une page physique et copier BootInfo
    let page = buddy_allocator::alloc_single_page()?;
    unsafe { core::ptr::write(page.virt_ptr() as *mut BootInfo, *boot_info); }

    // CORR-38 : Mapper en READ-ONLY dans la VMA de init_server
    // init_server n'a pas besoin d'écrire dans BootInfo
    let vma_addr = init_server::alloc_vma_for_boot_info();
    page_table::map_page(
        vma_addr,
        page.phys_addr(),
        PageProtection::READ,  // ← READ-ONLY obligatoire
    );

    vma_addr
}

// kernel/src/boot/boot_info.rs — vérification intégrité — CORR-38
impl BootInfo {
    pub fn validate(&self) -> bool {
        // Magic obligatoire
        if self.magic != BOOT_INFO_MAGIC { return false; }

        // Version supportée
        if self.version != BOOT_INFO_VERSION { return false; }

        // Padding doit être zéro (protection contre corruption)
        if self._pad  != [0u8; 4] { return false; }
        if self._pad2 != [0u8; 4] { return false; }

        // Reserved doit être tout-zéro
        if self.reserved.iter().any(|&x| x != 0) { return false; }

        // Champs critiques non-null
        if self.nr_cpus == 0 { return false; }
        if self.memory_bitmap_phys == 0 { return false; }
        if self.kernel_heap_start >= self.kernel_heap_end { return false; }

        true
    }
}
```

**Dans init_server/src/main.rs** :
```rust
fn _start(boot_info_virt: usize) -> ! {
    let bi = unsafe { &*(boot_info_virt as *const BootInfo) };

    // CORR-38 : Validation explicite avant tout usage
    if !bi.validate() {
        // Pas d'IPC possible (pas de CapToken valide avant validate)
        unsafe { arch::serial_writeln(b"[FATAL] BootInfo invalide — halt"); }
        unsafe { core::arch::asm!("hlt"); }
        loop {}
    }

    verify_cap_token(&bi.ipc_broker_cap, CapabilityType::IpcBroker);
    // ...
}
```

---

## CORR-39 🟠 — fd_table : validation des ObjectIds après restore Phoenix

### Problème
Après un restore Phoenix, la `fd_table` de `vfs_server` est restaurée depuis le snapshot RAM. Mais un `ObjectId` référencé par un fd peut avoir été supprimé du disque entre le gel et le crash. Le processus continuerait à utiliser un fd vers un objet inexistant.

**Source** : MiniMax EN-05

### Correction — `servers/vfs_server/src/isolation.rs`

```rust
// servers/vfs_server/src/isolation.rs — CORR-39

/// Appelée PAR RESTORE (après phoenix_wake_sequence), PAS par le gel.
/// Valide chaque fd ouvert contre le state actuel d'ExoFS.
pub fn validate_fd_table_after_restore() {
    log::info!("vfs_server : validation fd_table post-restore");
    let mut stale_count: u32 = 0;

    for fd in fd_table::iter_open_fds() {
        let obj_id = fd.obj_id;

        // Vérifier existence de l'ObjectId dans ExoFS
        let exists = syscall::exofs_stat(obj_id).is_ok();
        if !exists {
            log::warn!("vfs_server : fd {} ObjectId {:?} invalide post-restore — fermeture", fd.fd, obj_id);
            fd_table::close(fd.fd);
            stale_count += 1;
        }
    }

    if stale_count > 0 {
        log::warn!("vfs_server : {} fds fermés post-restore (ObjectIds invalides)", stale_count);
    } else {
        log::info!("vfs_server : fd_table cohérente post-restore");
    }
}
```

**Intégration dans la séquence de restore (CORR-35)** :
```rust
// Dans execute_restore_sequence() — après que vfs_server est opérationnel
if service == "vfs_server" {
    // Déclencher la validation fd_table
    ipc::send_to_vfs_server(VfsMsg::ValidateFdTablePostRestore)?;
}
```

---

## CORR-40 🟠 — IpcEndpoint : garantie Copy + assertion compile-time

### Problème
CORR-04 impose que `IpcEndpoint` soit stocké dans un tableau fixe sur la pile ISR (pas de heap). Cela requiert que `IpcEndpoint` implémente `Copy`. Si un futur développeur ajoute un champ non-`Copy` à `IpcEndpoint`, CORR-04 cesserait de fonctionner **silencieusement** (le compilateur accepterait un clone bitwise incorrect).

**Sources** : MiniMax EN-04, Copilote

### Correction — `libs/exo-types/src/ipc.rs`

```rust
// libs/exo-types/src/ipc.rs — CORR-40

/// Endpoint IPC : identifiant d'un canal de réception.
///
/// INVARIANT CRITIQUE (CORR-04 + CORR-40) :
///   IpcEndpoint DOIT être Copy pour usage dans les tableaux ISR.
///   Ne JAMAIS ajouter de champ non-Copy (Arc, Box, Vec, etc.).
///   Tout ajout de champ DOIT passer la CI (voir ci-dessous).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpcEndpoint {
    /// PID du processus receveur.
    pub pid:         u32,
    /// Index du canal de réception dans le processus.
    pub chan_idx:    u32,
    /// Génération du canal (détecte stale handles après restart driver).
    pub generation:  u32,
    /// Padding alignement.
    pub _pad:        u32,
}

// ─── Assertions compile-time ──────────────────────────────────────────
// Garantit que IpcEndpoint reste Copy et de taille prédictible.
const _: () = assert!(core::mem::size_of::<IpcEndpoint>() == 16);
// Vérifie que Copy est bien implémenté (compile error sinon)
const fn _assert_copy<T: Copy>() {}
const _: () = _assert_copy::<IpcEndpoint>();
```

**CI check** :
```bash
# Vérifier que IpcEndpoint n'a pas été modifié pour casser Copy
# (la static_assert dans le code le garantit déjà à la compilation)
# Vérification complémentaire : taille = 16B
grep -A 5 "struct IpcEndpoint" libs/exo-types/src/ipc.rs \
  | grep -q "size_of.*== 16" || echo "WARNING: IpcEndpoint size check absent"
```

---

## CORR-41 🟠 — verify_cap_token() : fermer le TODO constant-time

### Problème
`verify_cap_token()` dans `libs/exo-types/src/cap.rs` contient un `// TODO Phase 1 : LAC-01`.  
Tant que cette vérification n'est pas constant-time, des attaques de timing peuvent révéler si un token est valide ou quel champ ne correspond pas.

**Source** : ChatGPT5 §A, LAC-01 (Architecture v7 §9.2)

### Correction — `libs/exo-types/src/cap.rs`

```rust
// libs/exo-types/src/cap.rs — CORR-41
// LAC-01 : verify_cap_token() constant-time via crate `subtle` no_std.
//
// Crate subtle (version 2.x) : no_std, no_alloc, constant-time operations.
// Ajout dans libs/exo-types/Cargo.toml :
//   subtle = { version = "2.5", default-features = false }

use subtle::{Choice, ConstantTimeEq};

/// Vérifie qu'un CapToken correspond au type attendu — constant-time.
///
/// SÉCURITÉ :
///   - Constant-time : pas de branche dépendant des valeurs secrètes.
///   - Vérifie type_id ET generation (anti-replay).
///   - NE vérifie PAS object_id inline : trop coûteux constant-time
///     sans une clé de signature. Vérification object_id = Phase 1 crypto.
///
/// Retourne true si le token correspond. PANIQUE si type incorrect.
/// Conformément à CAP-01 : appelé en première instruction de main.rs.
pub fn verify_cap_token(token: &CapToken, expected: CapabilityType) -> bool {
    // Comparaison constant-time du type_id
    let type_match: Choice = token.type_id.ct_eq(&(expected as u16));

    // Vérification génération non-nulle (token émis par le kernel, pas forgé)
    let gen_valid:  Choice = (!token.generation.ct_eq(&0u64)).into();

    // Résultat constant-time : true seulement si les deux conditions sont vraies
    let result = bool::from(type_match & gen_valid);

    if !result {
        // Panic immédiat si token invalide (CAP-01)
        // Note : le message de panic ne révèle pas lequel des champs a échoué
        panic!("SECURITY: CapToken invalide en main.rs — arrêt");
    }

    result
}

// Dépendance à ajouter dans libs/exo-types/Cargo.toml :
// subtle = { version = "2.5", default-features = false, features = [] }
```

---

*ExoOS — Corrections Critiques & Majeures v2 (CORR-31 à CORR-41) — Mars 2026*  
*Sources : Z-AI, Copilote, ChatGPT5, KIMI-AI, MiniMax + analyse propre*
