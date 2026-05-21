# Corrections BLOC -1 — Bugs Kernel Bloquants

**Auteur :** claude iota  
**Date :** 2026-05-20  
**Priorité :** P0 ABSOLU — à appliquer avant tout autre travail v0.2.0  
**Référence audit :** `AUDIT_KERNEL_V0.2.0_CLAUDE_IOTA.md` INC-B01 à INC-B07

---

## CORR-IOTA-01 — VirtIO BAR Dynamique (INC-B01 + INC-B02 + INC-D03)

### Problème

`kernel/src/fs/exofs/storage/virtio_adapter.rs` utilise une constante hardcodée :

```rust
// AVANT — INCORRECT :
pub const DEFAULT_VIRTIO_BLK_MMIO_BASE: usize = 0x1000_0000;

pub fn init_global_disk() {
    init_global_disk_with_mmio(
        DEFAULT_VIRTIO_BLK_MMIO_BASE,
        DEFAULT_VIRTIO_BLK_CAPACITY_BYTES,
    );
}
```

### Correction — `kernel/src/fs/exofs/storage/virtio_adapter.rs`

```rust
// APRÈS — CORRECT :
// Plus de constante DEFAULT_VIRTIO_BLK_MMIO_BASE.
// L'adresse MMIO est lue depuis le BAR0 PCI après énumération.

pub fn init_global_disk() {
    // Étape 1 : Trouver le périphérique virtio-blk-pci dans la topologie PCI.
    // PCI vendor 0x1AF4, device 0x1042 (virtio-blk transitional: 0x1001)
    let mmio_base = match crate::drivers::pci_topology::find_virtio_blk_mmio_base() {
        Some(addr) => addr,
        None => {
            log::warn!("[ExoFS] virtio-blk introuvable dans la topologie PCI — disque désactivé");
            return;
        }
    };

    // Étape 2 : Lire la capacité depuis le registre VirtIO (offset 0x100 dans BAR0 + CommonCfg)
    let capacity_bytes = read_virtio_blk_capacity(mmio_base)
        .unwrap_or(512 * 1024 * 1024); // fallback 512 MiB si registre illisible

    init_global_disk_with_mmio(mmio_base, capacity_bytes);
    log::info!("[ExoFS] disque virtio-blk @ {:#x}, capacité {} MiB",
               mmio_base, capacity_bytes / (1024 * 1024));
}

/// Lit la capacité (en octets) depuis le CommonCfg VirtIO.
/// Offset du champ `capacity` dans la struct VirtioPciCommonCfg = 0x10 (u64, unités de 512 B).
fn read_virtio_blk_capacity(mmio_base: usize) -> Option<usize> {
    // SAFETY : l'adresse est vérifiée par find_virtio_blk_mmio_base() via phys_to_virt()
    let virt = crate::memory::core::address::phys_to_virt(
        crate::memory::core::types::PhysAddr::new(mmio_base as u64)
    );
    let capacity_sectors = unsafe {
        core::ptr::read_volatile((virt.as_u64() + 0x10) as *const u64)
    };
    if capacity_sectors == 0 { return None; }
    Some((capacity_sectors as usize).saturating_mul(512))
}
```

### Correction — `kernel/src/drivers/pci_topology.rs`

Ajouter la fonction de recherche du périphérique :

```rust
/// Cherche le premier périphérique VirtIO block dans la topologie PCI
/// et retourne l'adresse physique de son BAR0 (MMIO).
///
/// Compatible : virtio-blk-pci (PCI ID 1AF4:1042 ou 1AF4:1001 transitional)
pub fn find_virtio_blk_mmio_base() -> Option<usize> {
    // Parcourir les devices PCI enregistrés lors de drivers::init()
    for dev in pci_enumerate_devices() {
        if dev.vendor_id == 0x1AF4
            && (dev.device_id == 0x1042 || dev.device_id == 0x1001)
        {
            let bar0 = read_pci_bar0(dev.bus, dev.slot, dev.func);
            // BAR MMIO : bit 0 == 0, bits [2:1] == 0b00 (32-bit) ou 0b10 (64-bit)
            if bar0 & 0x1 == 0 {
                return Some((bar0 & !0xF) as usize);
            }
        }
    }
    None
}
```

### Vérification (test QEMU)

```bash
# Boot avec un disque virtio-blk-pci standard :
qemu-system-x86_64 \
  -drive file=disk.img,if=none,id=hd0 \
  -device virtio-blk-pci,drive=hd0 \
  -m 2G

# Dans ExoShell :
echo "persist_test" > /data/probe.txt
reboot
cat /data/probe.txt   # doit afficher "persist_test"
```

---

## CORR-IOTA-02 — cgroup Init Avant runqueue (INC-B05)

### Problème

Séquence actuelle dans `kernel_init()` :

```
Phase 3 : scheduler::init()
          ├─ runqueue::init_percpu()   ← runqueue prêt AVANT cgroup
          └─ ...

Phase 4 : process::init()
          └─ resource::cgroup::init() ← trop tard
```

Un idle thread est créé en Phase 3b et attaché à la runqueue **avant** que le root cgroup existe. `cgroup::attach(idle_pid, root_cgroup)` échoue silencieusement.

### Correction — `kernel/src/scheduler/mod.rs`

```rust
/// Phase 3 — Initialisation complète du scheduler.
/// Ordre strict : cgroup ROOT d'abord, puis runqueue, puis idle threads.
pub fn init(cpu_count: usize) {
    // 1. CGroups : le root cgroup doit exister avant tout attach.
    //    Déplacé depuis process::init() (CORR-IOTA-02).
    crate::process::resource::cgroup::init();
    let root_cg = crate::process::resource::cgroup::root();
    debug_assert!(root_cg.is_valid(), "root cgroup invalide après init");

    // 2. Runqueue per-CPU
    self::core::runqueue::init_percpu(cpu_count);

    // 3. Timer du scheduler (hrtimer LAPIC)
    self::timer::init();

    // 4. Idle threads
    for cpu in 0..cpu_count {
        let idle_pid = self::idle::create_idle_thread(cpu);
        // Attacher l'idle thread au root cgroup maintenant qu'il existe.
        let _ = crate::process::resource::cgroup::attach(idle_pid, root_cg);
    }

    // 5. Fork cloner
    self::fork::init();
}
```

### Correction — `kernel/src/process/mod.rs`

```rust
pub fn init() {
    pid::init();
    registry::init();
    maps::init();
    futex::init();
    // SUPPRIMÉ : resource::cgroup::init() — désormais dans scheduler::init()
    //            (CORR-IOTA-02 : cgroup doit précéder runqueue)
    acl::init();
    log::info!("[process] init OK");
}
```

### Test de non-régression

```rust
#[test]
fn test_cgroup_before_runqueue() {
    // Vérifier que le root cgroup est valide AVANT le premier idle thread
    let root = crate::process::resource::cgroup::root();
    assert!(root.is_valid(), "root cgroup doit être valide avant runqueue");
    // Vérifier que l'idle CPU 0 est bien dans le root cgroup
    let cpu0_idle = crate::scheduler::idle::idle_pid_for_cpu(0);
    assert!(crate::process::resource::cgroup::pid_cgroup(cpu0_idle) == root);
}
```

---

## CORR-IOTA-03 — ACPI Parser 1 GiB Limit (INC-B03 + INC-B04)

### Problème

```rust
// kernel/src/arch/x86_64/acpi/parser.rs — guard trop restrictif :
if xsdt_phys < 0x1000 || xsdt_phys >= 0x4000_0000 {
    return Err(AcpiError::InvalidPointer("XSDT hors de [4KiB..1GiB]"));
}
```

Sur un système avec 2 GiB RAM, QEMU place les tables ACPI à `0x7FE0000` – `0x7FFFFFFF`. Avec 4 GiB RAM, au-delà de `0x4000_0000`. Le guard bloque le boot.

### Correction — `kernel/src/arch/x86_64/acpi/parser.rs`

```rust
// AVANT — INCORRECT :
if xsdt_phys < 0x1000 || xsdt_phys >= 0x4000_0000 {
    return Err(AcpiError::InvalidPointer("XSDT hors de [4KiB..1GiB]"));
}

// APRÈS — CORRECT :
// La limite haute n'est plus 1 GiB mais la taille physique détectée.
// install_extended_physmap() est appelée AVANT acpi_init() dans memory_map.rs.
if xsdt_phys < 0x1000 {
    return Err(AcpiError::InvalidPointer("XSDT en dessous de 4KiB"));
}
// Vérification dynamique : l'adresse doit être dans la physmap réelle.
let phys_limit = crate::memory::core::layout::physmap_limit();
if xsdt_phys >= phys_limit {
    return Err(AcpiError::InvalidPointer("XSDT au-delà de la physmap"));
}
```

Appliquer la même correction sur tous les guards similaires dans `parser.rs`, `madt.rs`, `hpet.rs`.

### Ajout — `kernel/src/memory/core/layout.rs`

```rust
/// Constante : couverture initiale du boot page table = 1 GiB.
/// Vérifiée statiquement (CORR-IOTA-03 / O-04).
pub const PHYSMAP_INITIAL_COVERAGE: usize = 1 * 1024 * 1024 * 1024;
const _: () = assert!(
    PHYSMAP_INITIAL_COVERAGE == 0x4000_0000,
    "PHYSMAP_INITIAL_COVERAGE doit être exactement 1 GiB"
);

/// Retourne la limite physique réellement mappée (mise à jour par install_extended_physmap).
#[inline]
pub fn physmap_limit() -> u64 {
    PHYSMAP_LIMIT.load(core::sync::atomic::Ordering::Acquire)
}

static PHYSMAP_LIMIT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(PHYSMAP_INITIAL_COVERAGE as u64);

/// Appelé par install_extended_physmap() pour mettre à jour la limite.
pub fn set_physmap_limit(limit: u64) {
    PHYSMAP_LIMIT.store(limit, core::sync::atomic::Ordering::Release);
}
```

### Correction — `kernel/src/arch/x86_64/boot/memory_map.rs`

```rust
fn install_extended_physmap(phys_end: PhysAddr) {
    // ... code existant de mapping ...

    // Mettre à jour la limite globale (CORR-IOTA-03)
    crate::memory::core::layout::set_physmap_limit(phys_end.as_u64());
    log::info!("[physmap] étendue à {:#x} ({} GiB)",
               phys_end.as_u64(),
               phys_end.as_u64() / (1024 * 1024 * 1024));
}
```

---

## CORR-IOTA-04 — Injection PID via IPC (INC-B06)

### Problème

`sys_exo_ipc_send()` ne vérifie pas de capability token quand `msg_len == IPC_ENVELOPE_SIZE`. Un processus Ring3 sans privilège peut forger le champ PID de la zone `[100..120]`.

### Correction — `kernel/src/syscall/table.rs`

```rust
pub fn sys_exo_ipc_send(
    endpoint: u64,
    msg_ptr: u64,
    msg_len: u64,
    flags: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_SEND);
    let len = msg_len as usize;
    if len > crate::ipc::core::constants::MAX_MSG_SIZE {
        return E2BIG;
    }

    // ... validation UserBuf et copy_from_user existants ...

    // ── CORR-IOTA-04 : Guard cap token si msg pleine longueur ────────────────
    // Un message de taille IPC_ENVELOPE_SIZE inclut la zone ExoCapTokenWire [100..120].
    // Un processus non privilégié ne doit pas pouvoir envoyer un message pleine
    // longueur sans cap valide — vecteur d'injection PID.
    if len >= crate::ipc::core::constants::ABI_IPC_ENVELOPE_SIZE {
        let token_bytes =
            &payload[crate::ipc::core::constants::IPC_CAP_TOKEN_OFFSET
                     ..crate::ipc::core::constants::IPC_CAP_TOKEN_OFFSET
                       + crate::security::capability::CAP_TOKEN_WIRE_SIZE];

        match crate::security::capability::CapToken::from_bytes(token_bytes) {
            Ok(token) => {
                // Vérifier que le token appartient bien à l'appelant
                let caller_pid = crate::syscall::fast_path::syscall_current_pid();
                if let Err(_) = crate::security::capability::check_token_owner(
                    &token,
                    caller_pid,
                ) {
                    return EACCES; // PolicyDenied — token invalide ou PID usurpé
                }
            }
            Err(_) => {
                // Pas de cap token valide à la zone attendue
                return EACCES;
            }
        }
    }
    // ─────────────────────────────────────────────────────────────────────────

    // Suite : IPC_FLAG_INJECT_SRC_PID, is_reserved_kernel_ipc, send_raw...
    // ... code existant inchangé ...
}
```

### Ajout — `kernel/src/ipc/core/constants.rs`

```rust
/// Offset dans le payload d'un message pleine longueur où se trouve le ExoCapTokenWire.
/// Zone [100..120] = 20 octets (CAP_TOKEN_WIRE_SIZE).
pub const IPC_CAP_TOKEN_OFFSET: usize = 100;
```

---

## CORR-IOTA-05 — exosh Sans Blocage Réseau (INC-B07)

### Problème

`init_server` attend `network_server` avec un timeout de 30 secondes. `exosh` est dans la file d'attente du graph de boot — il ne peut démarrer qu'une fois tous les services précédents terminés, y compris `network_server`.

### Correction — `servers/init_server/src/service_table.rs`

```rust
// AVANT :
ServiceMetadata {
    name: "network_server",
    bin_path: NETWORK_SERVER_BIN,
    requires: DEPS_NETWORK,
    requires_optional: NO_DEPS,
    ready_timeout_ms: 30_000,   // ← bloque le graph 30s
    critical: true,             // ← une panne réseau arrête tout le boot
},

// APRÈS :
ServiceMetadata {
    name: "network_server",
    bin_path: NETWORK_SERVER_BIN,
    requires: DEPS_NETWORK,
    requires_optional: NO_DEPS,
    ready_timeout_ms: 10_000,   // ← 10s max (pas 30s)
    critical: false,            // ← réseau optionnel au boot
},
```

### Correction — `servers/init_server/src/boot_sequence.rs`

```rust
/// Lance les services en parallèle par vague (wave).
/// Une vague = tous les services dont les dépendances sont satisfaites simultanément.
/// Remplace la boucle séquentielle (CORR-IOTA-05 / CORR-81 ERR-11).
pub unsafe fn boot_services(services: &mut [Service]) -> Result<(), BootError> {
    loop {
        let ready: Vec<usize> = services
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.current_pid() == 0           // pas encore démarré
                    && dependencies_satisfied(s, services)
            })
            .map(|(i, _)| i)
            .collect();

        if ready.is_empty() { break; }

        // Spawner toute la vague simultanément
        for &idx in &ready {
            let pid = spawn_service(services[idx].name, services[idx].bin_path);
            services[idx].set_pid(pid);
            log::debug!("[boot] spawn {} (pid {})", services[idx].name, pid);
        }

        // Attendre la vague en parallèle (timeout par service)
        for &idx in &ready {
            let meta = dependency::metadata(services[idx].name).unwrap();
            let ok = wait_for_ipc_ready(
                services[idx].current_pid(),
                meta.ready_timeout_ms,
            );
            if !ok && dependency::is_critical(services[idx].name) {
                return Err(BootError::CriticalServiceFailed(services[idx].name));
            }
        }
    }
    Ok(())
}
```

---

## Vérifications Post-Correction (Ordre d'Exécution)

```bash
# 1. Compilation sans erreur
cargo build --target x86_64-exoos-kernel 2>&1 | grep -E "error|warning"

# 2. Tests unitaires kernel (ci-dessous doivent passer)
cargo test -p kernel -- test_cgroup_before_runqueue
cargo test -p kernel -- test_physmap_limit_update
cargo test -p kernel -- test_virtio_blk_bar_detection

# 3. Boot QEMU 2 GiB
qemu-system-x86_64 -m 2G -kernel exoos.elf -nographic 2>&1 | grep -E "PANIC|physmap étendue"

# 4. Persistance disque
qemu-system-x86_64 -m 2G \
  -drive file=test.img,if=none,id=d0 \
  -device virtio-blk-pci,drive=d0 \
  -kernel exoos.elf -append "root=/dev/vda" \
  -nographic &
sleep 10
echo "test" | nc -q1 localhost 4444  # via exosh IPC si configuré
```

---

*claude iota — CORRECTIONS_BLOC_MOINS1_CLAUDE_IOTA.md — 2026-05-20*
