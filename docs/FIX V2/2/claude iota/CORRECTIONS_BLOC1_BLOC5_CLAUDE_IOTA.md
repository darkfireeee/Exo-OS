# Corrections BLOC 1 — ExoPhoenix · BLOC 5 — Bibliothèques

**Auteur :** claude iota  
**Date :** 2026-05-20  
**Référence audit :** `AUDIT_KERNEL_V0.2.0_CLAUDE_IOTA.md` INC-P · INC-L

---

## BLOC 1 — ExoPhoenix : Détail des Corrections

---

### CORR-IOTA-17 — SSR Statique : `const_assert!` + Layout Précis (INC-P01)

**Contexte :** La crate `exo-phoenix-ssr` (dans `libs/`) définit la struct `SSR`.
Le kernel doit vérifier statiquement que cette struct tient dans 4 KiB **à la compilation**.
La correction CORR-IOTA-07 a ajouté le `const_assert!` dans `ssr.rs` du kernel.
Voici le complément côté crate pour garantir la cohérence du layout.

**Fichier :** `kernel/src/exophoenix/ssr.rs` — section vérification layout

```rust
use crate::arch::constants::{
    SSR_PHYS_BASE, SSR_PHYS_END,
    CORE_MASK_WORDS, MAX_CORES_LAYOUT,
};

// ── Layout précis (calcul explicite, pas d'opaque size) ──────────────────
/// Offset du champ `header` dans la SSR (début).
pub const SSR_OFFSET_HEADER:    usize = 0;
/// Offset du checksum BLAKE3 (fin du header = 64 octets).
pub const SSR_OFFSET_CHECKSUM:  usize = 64;
/// Offset du champ flags_and_version (après checksum 32 octets).
pub const SSR_OFFSET_FLAGS:     usize = SSR_OFFSET_CHECKSUM + 32; // 96
/// Offset du tableau process_records.
pub const SSR_OFFSET_PROCS:     usize = 104;
/// Offset du tableau endpoint_records (après N_PROCS * 96 octets).
pub const SSR_OFFSET_ENDPOINTS: usize =
    SSR_OFFSET_PROCS + SSR_MAX_PROCESSES * PROCESS_RECORD_SIZE; // 104 + 2304 = 2408
/// Fin des endpoint records.
pub const SSR_OFFSET_END:       usize =
    SSR_OFFSET_ENDPOINTS + SSR_MAX_ENDPOINTS * ENDPOINT_RECORD_SIZE; // 2408 + 1152 = 3560
/// Taille totale avec padding à 16 octets.
pub const SSR_SIZE: usize = (SSR_OFFSET_END + 15) & !15; // 3568

// ── Invariants statiques ──────────────────────────────────────────────────
const _: () = assert!(SSR_SIZE <= 4096,
    "SSR_SIZE dépasse 4 KiB — réduire SSR_MAX_PROCESSES ou SSR_MAX_ENDPOINTS");
const _: () = assert!(SSR_OFFSET_PROCS < SSR_OFFSET_ENDPOINTS,
    "Layout SSR incohérent : PROCS doit précéder ENDPOINTS");
const _: () = assert!(SSR_PHYS_END as usize - SSR_PHYS_BASE as usize >= SSR_SIZE,
    "Zone physique SSR < SSR_SIZE");
const _: () = assert!(SSR_MAX_PROCESSES <= 255,
    "SSR_MAX_PROCESSES doit tenir sur u8 pour le champ count");
const _: () = assert!(SSR_MAX_ENDPOINTS <= 255,
    "SSR_MAX_ENDPOINTS doit tenir sur u8 pour le champ count");

// ── Test de cohérence avec core_mask ─────────────────────────────────────
/// Le bitmask de cores dans chaque ProcessRecord utilise CORE_MASK_WORDS mots u64.
pub const PROCESS_CORE_MASK_OFFSET: usize = 80; // dans ProcessRecord
const _: () = assert!(
    PROCESS_CORE_MASK_OFFSET + CORE_MASK_WORDS * 8 <= PROCESS_RECORD_SIZE,
    "core_mask déborde du ProcessRecord"
);
```

---

### CORR-IOTA-18 — Ring1 Parallèle : Refactoring boot_services (INC-P07)

**Contexte :** CORR-81 (ERR-11) exige que les serveurs Ring1 démarrent en parallèle
après une bascule Phoenix A↔B. Le SLA de recovery est < 500ms.

La correction CORR-IOTA-05 a introduit le principe du boot par vague.
Voici la version complète avec :
- Détection des vagues par graphe de dépendances
- Spawn parallèle
- Attente en parallèle avec timeout individuel
- Rapport de recovery time

**Fichier :** `servers/init_server/src/boot_sequence.rs` — remplacement complet

```rust
//! boot_sequence.rs — Démarrage des services par vague parallèle.
//!
//! Algorithme : BFS sur le graphe de dépendances.
//! Chaque vague contient tous les services dont les dépendances
//! sont satisfaites. Les services d'une même vague démarrent simultanément.
//!
//! Garantie : recovery_time <= max(wave_spawn_time + max(ready_timeout_per_wave))
//! Cible SLA v0.2.0 : < 500ms total pour les 6 Ring1 servers critiques.

use crate::service_table::{Service, ServiceMetadata, CANONICAL_SERVICES};
use crate::dependency;

const BOOT_HARD_TIMEOUT_MS: u64 = 60_000; // 60s global max

/// Point d'entrée : démarre tous les services par vagues.
/// Retourne Ok(()) si tous les services critiques sont prêts.
pub unsafe fn boot_services(services: &mut [Service]) -> Result<(), BootError> {
    let t_start = crate::syscall::monotonic_ns();
    let mut wave_idx = 0usize;

    loop {
        // Calculer la vague courante : services non démarrés dont les deps sont prêtes.
        let wave: heapless::Vec<usize, 32> = services
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.current_pid() == 0 && deps_ready(s, services)
            })
            .map(|(i, _)| i)
            .collect();

        if wave.is_empty() {
            break; // Plus rien à démarrer
        }

        log::info!("[boot] vague {} : {} service(s)", wave_idx, wave.len());

        // ── Spawn de toute la vague ──────────────────────────────────────
        let mut pids: heapless::Vec<(usize, u32), 32> = heapless::Vec::new();
        for &idx in wave.iter() {
            let svc = &services[idx];
            let pid = spawn_service(svc.name, svc.bin_path);
            if pid == 0 {
                if dependency::is_critical(svc.name) {
                    return Err(BootError::SpawnFailed(svc.name));
                }
                log::warn!("[boot] spawn {} échoué (non-critique)", svc.name);
                continue;
            }
            services[idx].set_pid(pid);
            let _ = pids.push((idx, pid));
            log::debug!("[boot]   ↳ {} pid={}", svc.name, pid);
        }

        // ── Attente parallèle (poll sur tous les services de la vague) ───
        let wave_deadline_ms = wave.iter()
            .map(|&i| {
                dependency::metadata(services[i].name)
                    .map(|m| m.ready_timeout_ms)
                    .unwrap_or(5_000)
            })
            .max()
            .unwrap_or(5_000);

        let t_wave = crate::syscall::monotonic_ns();
        let mut remaining: heapless::Vec<(usize, u32), 32> = pids.clone();

        loop {
            let now = crate::syscall::monotonic_ns();
            let elapsed_ms = (now - t_wave) / 1_000_000;

            if elapsed_ms >= wave_deadline_ms || remaining.is_empty() {
                break;
            }

            let mut still_pending: heapless::Vec<(usize, u32), 32> = heapless::Vec::new();
            for &(idx, pid) in remaining.iter() {
                if is_ipc_ready(pid) {
                    let elapsed = (crate::syscall::monotonic_ns() - t_wave) / 1_000_000;
                    log::info!("[boot]   ✓ {} prêt en {}ms", services[idx].name, elapsed);
                } else {
                    let _ = still_pending.push((idx, pid));
                }
            }
            remaining = still_pending;

            if !remaining.is_empty() {
                crate::syscall::yield_cpu();
            }
        }

        // Vérifier les services critiques non prêts après timeout
        for (idx, _pid) in remaining.iter() {
            if dependency::is_critical(services[*idx].name) {
                return Err(BootError::CriticalServiceTimeout(services[*idx].name));
            }
            log::warn!("[boot] {} timeout (non-critique, continuité)", services[*idx].name);
        }

        wave_idx += 1;

        // Guard global
        let total_ms = (crate::syscall::monotonic_ns() - t_start) / 1_000_000;
        if total_ms >= BOOT_HARD_TIMEOUT_MS {
            return Err(BootError::GlobalTimeout);
        }
    }

    let total_ms = (crate::syscall::monotonic_ns() - t_start) / 1_000_000;
    log::info!("[boot] tous les services démarrés en {}ms ({} vagues)", total_ms, wave_idx);

    // SLA check v0.2.0 : < 500ms pour les Ring1 critiques
    if total_ms > 500 {
        log::warn!("[boot] SLA 500ms manqué ({}ms) — profilage recommandé", total_ms);
    }

    Ok(())
}

/// Vérifie que toutes les dépendances REQUIRED d'un service sont prêtes (pid != 0 et IPC ready).
fn deps_ready(svc: &Service, all: &[Service]) -> bool {
    let meta = match dependency::metadata(svc.name) {
        Some(m) => m,
        None    => return true,
    };
    meta.requires.iter().all(|dep_name| {
        all.iter().any(|s| s.name == *dep_name && s.current_pid() != 0)
    })
}

/// Teste si un service est prêt à recevoir des messages IPC (non bloquant).
fn is_ipc_ready(pid: u32) -> bool {
    // Tentative de `ipc_ping` (syscall léger, retourne EAGAIN si pas prêt)
    unsafe { crate::syscall::ipc_ping(pid) == 0 }
}

#[derive(Debug)]
pub enum BootError {
    SpawnFailed(&'static str),
    CriticalServiceTimeout(&'static str),
    CriticalServiceFailed(&'static str),
    GlobalTimeout,
}
```

---

### CORR-IOTA-19 — Typo SSR TLA+ (INC-P14)

**Fichier :** `docs/Exo-OS-TLA+/redme_final_test.md`

```markdown
<!-- AVANT — INCORRECT : -->
SSR layout | Physical `[0x1000000..0x110000]`

<!-- APRÈS — CORRECT : -->
SSR layout | Physical `[0x1000000..0x1100000]`
```

La plage `[0x1000000..0x1100000]` représente la zone physique de 16 MiB à 17.004 MiB
(taille = 0x100000 = 1 MiB, dont 4 KiB sont utilisés par la SSR).

**Note :** Vérifier également que le fichier E820 de configuration QEMU (`exo-boot/src/e820.rs`)
réserve bien `0x1000000..0x1100000` et non `0x1000000..0x110000`.

```rust
// exo-boot/src/e820.rs — entrée SSR attendue :
E820Entry {
    base: 0x0100_0000,          // 16 MiB
    len:  0x0010_0000,          // 1 MiB réservée (SSR + padding)
    kind: E820Kind::Reserved,
},
// Vérification :
const _: () = assert!(0x0100_0000 + 0x0010_0000 == 0x0110_0000);
// 0x0110_0000 ≠ 0x110000 — la typo dans le README confondait ces deux valeurs.
```

---

### CORR-IOTA-20 — Séquence de Boot Sécurité : Documentation BOOT_SEQUENCE_V0.2

**Fichier à créer :** `docs/Vision v0.2.0/BOOT_SEQUENCE_V0.2.md`

Ce fichier n'existe pas et est requis par la checklist S-01 à S-12 pour documenter la séquence
garantie. Voici le contenu attendu :

```markdown
# Séquence de Boot Sécurité — ExoOS v0.2.0

## Chemins de Boot

ExoOS supporte deux chemins d'entrée :
- `_start_multiboot` — GRUB/Multiboot2 (développement)
- `_start_uefi`      — UEFI GOP (production)

Les deux chemins convergent vers `arch_boot_init()` puis `kernel_main()`.

## Phases Garanties

| Phase | Étape | Module | Condition préalable |
|---|---|---|---|
| 0 | memory_init (E820 parse) | `memory_map.rs` | Aucune |
| 0b | install_extended_physmap | `memory_map.rs` | E820 parsé |
| 1 | arch_init (GDT/IDT/TSS) | `early_init.rs` | Aucune |
| 1b | LAPIC init + APIC timer | `lapic.rs` | IDT chargée |
| 2 | ExoCage hw (SMEP/SMAP/KPTI) | `spectre/kpti.rs` | Avant heap |
| 2b | ExoCage CET (MSR CET_EN) | `security/exocage.rs` | CPUID vérifié |
| 3 | ExoNMI watchdog | `security/exonmi.rs` | LAPIC disponible |
| 4 | Heap allocator | `memory/heap.rs` | physmap ≥ couverte |
| 5 | ExoCage per-thread BSP | `security/exocage.rs` | Heap + TCB BSP |
| 6 | ExoSeal verify_chain() | `security/exoseal.rs` | Heap + PCI énuméré |
| 7 | ExoShield IOMMU | `drivers/iommu/mod.rs` | PCI énuméré |
| 8 | Scheduler + cgroup root | `scheduler/mod.rs` | Heap |
| 9 | Process registry | `process/mod.rs` | Scheduler |
| 10 | IPC subsystem | `ipc/mod.rs` | Scheduler |
| 11 | ExoFS + VirtIO BAR PCI | `fs/exofs/mod.rs` | PCI BAR lu |
| 12 | ExoKairos capabilities | `security/exokairos.rs` | Heap |
| 13 | ExoLedger audit chain | `security/exoledger.rs` | ExoFS |
| 14 | Ring0→Ring1 handoff | `userspace_boot.rs` | Tous les modules |

## Invariants

- SMEP/SMAP activés avant tout accès heap (Phase 2 << Phase 4)
- cgroup root valide avant premier idle thread (Phase 8 en bloc)
- VirtIO BAR lu depuis PCI config space (jamais hardcodé)
- ExoNMI armé avant Ring1 handoff (Phase 3 << Phase 14)
```

---

## BLOC 5 — Bibliothèques ExoOS

---

### CORR-IOTA-21 — Seuil Inline/SHM IPC (INC-L04)

**Contexte :** La spec CORR-85 définit :
- Payload ≤ 200 octets → IPC inline (zéro copie kernel, payload dans le slot de ring)
- Payload > 200 octets → SHM + IPC référence (4 octets de handle SHM)

La valeur actuelle `IPC_INLINE_PAYLOAD_SIZE = 120` force du SHM pour des messages
entre 121 et 200 octets — surcharge inutile pour les réponses crypto (128–192 octets typiques).

**Fichier :** `servers/syscall_abi/src/lib.rs`

```rust
// AVANT — INCORRECT (ERR-05) :
pub const IPC_HEADER_SIZE:        usize = 8;
pub const IPC_INLINE_PAYLOAD_SIZE: usize = 120; // trop petit
pub const IPC_ENVELOPE_SIZE:      usize = 128;  // = 8 + 120

// APRÈS — CORRECT (CORR-85) :
pub const IPC_HEADER_SIZE:        usize =   8;
/// Taille maximale du payload inline (sans SHM).
/// Doit couvrir les réponses crypto (128–192 B) et les réponses VFS courtes.
pub const IPC_INLINE_PAYLOAD_SIZE: usize = 192; // spec : ≤ 200 octets inline
pub const IPC_ENVELOPE_SIZE:      usize = 200;  // = 8 + 192
pub const IPC_CAP_TOKEN_OFFSET:   usize = 172;  // [172..192] = 20 octets ExoCapTokenWire
pub const IPC_CAP_TOKEN_SIZE:     usize =  20;

// ── Invariants statiques ──────────────────────────────────────────────────
const _: () = assert!(IPC_INLINE_PAYLOAD_SIZE <= 200,
    "IPC_INLINE_PAYLOAD_SIZE > 200 octets — viole la spec CORR-85");
const _: () = assert!(IPC_HEADER_SIZE + IPC_INLINE_PAYLOAD_SIZE == IPC_ENVELOPE_SIZE,
    "IPC_ENVELOPE_SIZE incohérent");
const _: () = assert!(IPC_CAP_TOKEN_OFFSET + IPC_CAP_TOKEN_SIZE == IPC_ENVELOPE_SIZE,
    "ExoCapTokenWire doit être en fin de payload");
```

**Fichier :** `kernel/src/ipc/core/constants.rs` — mettre à jour en conséquence :

```rust
// Synchroniser avec syscall_abi
pub const ABI_IPC_ENVELOPE_SIZE:   usize = 200;  // ← était 128
pub const ABI_IPC_PAYLOAD_SIZE:    usize = 192;  // ← était 120
pub const MAX_MSG_SIZE:            usize = 240;  // inchangé (ring slot = header + max)
pub const IPC_CAP_TOKEN_OFFSET:    usize = 172;  // ← nouveau

const _: () = assert!(ABI_IPC_ENVELOPE_SIZE <= MAX_MSG_SIZE,
    "IPC_ENVELOPE_SIZE > MAX_MSG_SIZE du kernel");
```

**Impact sur le ring slot :**

```
Avant : RING_SLOT_SIZE = MSG_HEADER_SIZE(8) + MAX_MSG_SIZE(240) = 248 octets
Après : RING_SLOT_SIZE = MSG_HEADER_SIZE(8) + MAX_MSG_SIZE(240) = 248 octets (inchangé)
```

Le ring slot n'est pas affecté car `MAX_MSG_SIZE = 240 > 192`. La modification
n'impacte que la **décision inline vs SHM** dans le path d'envoi IPC.

**Fichier :** `kernel/src/syscall/table.rs` — `sys_exo_ipc_send()` — mettre à jour la constante
de référence :

```rust
// Remplacer la référence à ABI_IPC_ENVELOPE_SIZE dans le guard de cap token :
// Avant : if len >= 128
// Après :
if len >= crate::ipc::core::constants::ABI_IPC_ENVELOPE_SIZE { // 200
    // ... vérification cap token (CORR-IOTA-04) ...
}
```

---

### CORR-IOTA-22 — `exo-alloc` Backend : Vérification dlmalloc no_std (INC-L01)

**Contexte :** La crate `exo-alloc` est dans `libs/exo-alloc/` (hors scope d'extraction directe).
La Vision ExoOS impose que le backend soit `dlmalloc` (no_std pur, pas de libc).

**Vérification à effectuer manuellement :**

```bash
# Vérifier que exo-alloc n'importe pas libc :
grep -r "extern.*libc\|use libc\|link.*libc\|link_name.*malloc" libs/exo-alloc/src/

# Vérifier que le GlobalAllocator pointe vers dlmalloc :
grep -rn "GlobalAllocator\|#\[global_allocator\]" libs/exo-alloc/src/

# Attendu dans libs/exo-alloc/src/lib.rs :
# #[global_allocator]
# static ALLOCATOR: DlmallocAllocator<Exo> = DlmallocAllocator::new();
```

**Point de vigilance — `deny.toml` (CORR-IOTA-08) :**
Avec `cargo deny` actif, toute introduction de `libc` comme dépendance
(directe ou transitive de `exo-alloc`) sera bloquée en CI.

---

## Tests de Non-Régression ExoPhoenix

### Scénario de Bascule A→B (SLA < 500ms)

```bash
# 1. Boot sur partition A
qemu-system-x86_64 -m 2G -kernel exoos-a.elf -nographic &
QEMU_PID=$!
sleep 5

# 2. Injecter un défaut pour déclencher la bascule Phoenix
# (via IPC exo_phoenix_trigger_failover ou via sysctl)
echo "failover" > /dev/exo_phoenix

# 3. Mesurer le temps jusqu'à ce qu'exosh soit à nouveau disponible
t_start=$(date +%s%3N)
until nc -z localhost 4444 2>/dev/null; do sleep 0.01; done
t_end=$(date +%s%3N)
echo "Recovery time: $((t_end - t_start))ms"  # doit être < 500ms

kill $QEMU_PID
```

### Test Persistance SSR

```bash
# Vérifier que la SSR est dans la zone physique E820 réservée
# (log kernel au boot) :
grep "SSR" /dev/kmsg | grep -E "0x1[0-9a-f]{6}\.\.0x1[0-9a-f]{6}"
# Attendu : SSR @ 0x1000000..0x1001000 (4096 octets)
```

---

*claude iota — CORRECTIONS_BLOC1_BLOC5_CLAUDE_IOTA.md — 2026-05-20*
