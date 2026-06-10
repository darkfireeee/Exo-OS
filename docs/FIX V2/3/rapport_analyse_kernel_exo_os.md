# Rapport d'analyse du kernel Exo-OS

**Dépôt analysé :** `https://github.com/darkfireeee/Exo-OS.git`  
**Version :** v0.1.0 "Elder and Bobby"  
**Date d'analyse :** 2026-06-05  
**Cible :** `x86_64-unknown-none` (bare-metal)  
**Langage :** Rust `no_std` / `no_main`  
**Périmètre :** `kernel/src/` — 748 fichiers `.rs`, ~283 400 lignes de code

---

## Table des matières

1. [Vue d'ensemble architecturale](#1-vue-densemble-architecturale)
2. [Séquence de démarrage](#2-séquence-de-démarrage)
3. [Sous-systèmes clés](#3-sous-systèmes-clés)
4. [Incohérences critiques](#4-incohérences-critiques)
5. [Incohérences structurelles](#5-incohérences-structurelles)
6. [Incohérences mineures](#6-incohérences-mineures)
7. [Tableau récapitulatif](#7-tableau-récapitulatif)
8. [Recommandations](#8-recommandations)

---

## 1. Vue d'ensemble architecturale

Exo-OS est un **microkernel hybride** : le kernel gère directement la mémoire, le scheduler et les appels système (traits monolithiques), mais délègue les services système à 13 serveurs Ring1 indépendants (traits microkernel). Il cible exclusivement `x86_64` à ce stade ; le répertoire `arch/aarch64/` est un placeholder déclaré non supporté.

### Couches du kernel (ordre strict de dépendance)

```
Transverse : arch/         ← peut appeler toutes les couches
Couche 0   : memory/       ← aucune dépendance kernel
Couche 1   : scheduler/    ← dépend de memory/
Couche 1.5 : process/      ← dépend de memory/ + scheduler/
Couche 2a  : ipc/          ← dépend de memory/ + scheduler/ + process/
Couche 2b  : security/     ← dépend de memory/ + scheduler/ + process/
Couche 3   : fs/           ← dépend de tout
Transverse : drivers/      ← PCI, DMA, IOMMU, VirtIO
Transverse : syscall/      ← dispatch, validation, bridges
Transverse : exophoenix/   ← dual-kernel fault-tolerance (Kernel A + B)
```

L'ordre des verrous (`lock ordering`) suit la même hiérarchie :
`Memory → Scheduler → Security → IPC → FS`

### Serveurs Ring1

Treize serveurs sont chargés statiquement depuis ExoFS au démarrage :
`init_server`, `ipc_router`, `memory_server`, `vfs_server`, `crypto_server`, `device_server`, `virtio_drivers`, `network_server`, `scheduler_server`, `input_server`, `tty_server`, `exo_shield`, `exosh`.

### Suite sécurité ExoShield

| Module | Mécanisme hardware |
|---|---|
| ExoCage | Intel CET — Shadow Stack + IBT |
| ExoVeil | PKS (Protection Key Supervisor) |
| ExoLedger | Audit BLAKE3 chaîné, zone P0 immuable |
| ExoKairos | Capabilities temporelles à deadline chiffrée |
| ExoArgos | Surveillance comportementale, score de menace |
| ExoNMI | Handler NMI dédié, escalade vers ExoPhoenix |
| ExoSeal | Verrouillage CET/PKS/IOMMU pré-scheduler |

---

## 2. Séquence de démarrage

### Chemin GRUB Multiboot2 (défaut)

```
GRUB _start (32-bit protégé, EAX=magic, EBX=info)
  └─ Trampoline 32→64 bits (ASM inline Rust)
       ├─ Construction PML4 + PDPT + PD (identity 0..1 GiB, huge pages 2 MiB)
       ├─ Mapping MMIO LAPIC 0xFEE00000, IOAPIC 0xFEC00000 (PDPT[3])
       ├─ LGDT GDT 64-bit minimale
       ├─ CR3=PML4, CR4.PAE=1, EFER.LME=1, CR0.PG=1
       └─ RETF vers CS=0x08 → _start64

_start64
  └─ kernel_main(mb2_magic, mb2_info, rsdp_phys)
       ├─ Phase 1  : arch_boot_init()       [GDT, IDT, TSS, APIC, SMP, Spectre]
       ├─ Phase 2a : EmergencyPool          [RÈGLE EMERGENCY-01, premier absolu]
       ├─ Phase 2b : Heap hybride (SLUB)    [+ CoW init + LAPIC fixmap]
       ├─ Phase 2c : time_init()            [HPET + calibration TSC + seqlock]
       ├─ Phase 2d : drivers::init()        [IOMMU queues GI-03]
       ├─ Phase 2e : exoseal_boot_phase0()  [CET/PKS/IOMMU avant runqueues]
       ├─ Phase 2f : cgroup root
       ├─ Phase 3  : scheduler::init()      [runqueues per-CPU, idle threads]
       ├─ Phase 3b : idle threads BSP+APs
       ├─ Phase 3c : register addr_space_cloner (fork)
       ├─ Phase 4  : process::init()        [PID table 32 768 slots, reaper]
       ├─ Phase 5  : security_init()        [capabilities, crypto, isolation, audit]
       ├─ Phase 6  : ipc_init()             [SPSC rings, pool SHM 1 MiB, hooks VMM]
       ├─ Phase 7  : exofs_init()           [VirtIO-blk, ELF loader, fs_bridge]
       └─ boot_userspace()                  [/sbin/exo-init-server → PID 1]
```

### Chemin exo-boot (UEFI)

`exo-boot` entre directement en 64 bits (`_start_uefi`), sauvegarde `EAX`/`RBX`, et rejoint `_start64`. Le reste du chemin est identique.

---

## 3. Sous-systèmes clés

### Mémoire

- **Buddy allocator** pour les frames physiques (zones physiques, NUMA-aware).
- **SLUB** pour les petits objets, **heap hybride** pour les allocations dynamiques.
- **CoW lock-free**, hugepages 2 MiB, swap CLOCK, KASAN-lite, protection NX/SMEP/SMAP.
- **Futex table** : singleton unique défini dans `memory/utils/`, graine SipHash anti-DoS initialisée après `security_init()`.

### Scheduler

Runqueues per-CPU (clamp à `MAX_CPUS = 64`), politiques CFS/RT/IDLE/EDF, context switch ASM (`switch_asm.s` inclus via `global_asm!`), FPU lazy, énergie C-states, timer HPET+TSC à HZ=1000.

### TCB — Layout canonique 256 octets

```
[0]   tid : u64
[8]   kstack_ptr : u64          ← HARDCODÉ switch_asm.s
[16]  priority, policy
[24]  sched_state : AtomicU64
[56]  cr3_phys : u64            ← HARDCODÉ switch_asm.s
[92]  pid : ProcessId           ← HARDCODÉ signal/compat
[144] _cold_reserve : [u8; 88]  ← ExoShield v1.0
  [0..7]  shadow_stack_token    (offset absolu TCB 144)
  [8]     cet_flags             (offset absolu TCB 152)
  [9]     threat_score_u8       (offset absolu TCB 153)
  [16..23] pt_buffer_phys       (offset absolu TCB 160)
[232] fpu_state_ptr : u64       ← HARDCODÉ ExoPhoenix
[240] rq_next : u64
[248] rq_prev : u64
```

Des `const_assert!` (`offset_of!`, `size_of!`) vérifient ces offsets à la compilation.

### IPC

Rings SPSC/MPMC zéro-copie (16 slots de 256 octets chacun), endpoints (max 8 192), canaux (max 65 536), mémoire partagée NUMA-aware (pool 1 MiB alloué au boot), RPC avec timeout (5 ms défaut, 100 ms max), bridge capability.

### ExoPhoenix

Architecture dual-kernel : Kernel A (production) est surveillé par Kernel B (sentinelle). En cas de crash, B verrouille l'IOMMU, vérifie l'image via un contrat hash BLAKE3 (`Forge`), recharge A depuis ExoFS, et reprend l'exécution. Onze états atomiques régissent la machine :

```
BootStage0 → Normal → Threat → IsolationSoft → IsolationHard
           → Certif → Restore → Degraded / Emergency
           → NetworkDraining → NetworkSerialized
```

---

## 4. Incohérences critiques

### 4.1 `stage0_init_all_steps()` n'est jamais appelé

**Fichiers concernés :** `exophoenix/stage0.rs`, `lib.rs`

`stage0_init_all_steps()` configure l'ensemble du sous-système ExoPhoenix côté Kernel B : attache tous les devices PCI au domaine IOMMU bloquant, stocke le hash MADT, arme le watchdog, initialise le pool R3. Cette fonction est définie à la ligne 1001 de `stage0.rs` mais **aucun chemin dans `kernel_init()` ni dans `kernel_main()` ne l'appelle**.

```rust
// stage0.rs ligne 1001
pub fn stage0_init_all_steps() -> Stage0Summary { ... }

// lib.rs — kernel_init() ne contient aucun appel à stage0
// grep résultat : zéro occurrence hors stage0.rs
```

**Conséquence directe :** la variable `IOMMU_BLOCKED_DOMAIN_ID` est initialisée à `0` et reste à `0` tout au long du boot normal.

```rust
// stage0.rs ligne 156
static IOMMU_BLOCKED_DOMAIN_ID: AtomicU32 = AtomicU32::new(0);
```

Si un handoff ExoPhoenix se déclenche (crash de Kernel A), `stage_soft_revoke_iommu()` appelle `stage0::blocked_domain_id()` qui retourne `0`, puis flush le domaine IOMMU numéro zéro. Le domaine 0 est en général le domaine d'identité par défaut de VT-d, **pas** le domaine de blocage prévu. L'isolation IOMMU censée protéger le handoff est silencieusement inopérante.

---

### 4.2 Contrat ExoPhoenix nul dans tout build standard

**Fichiers concernés :** `kernel/build.rs`, `exophoenix/forge.rs`

Le `build.rs` a trois chemins pour fournir les artefacts ExoPhoenix à Kernel B :

1. `KERNEL_A_IMAGE_PATH` défini → image lue, hash BLAKE3 calculé, Merkle calculé. ✅
2. `EXOPHOENIX_BUILD_ROLE=A` → artefacts écrits à zéro (première passe). ✅ (volontaire)
3. **Aucune variable définie (cas par défaut)** → `write_artifacts(&out, &[], &ZERO_HASH, &ZERO_HASH)`. ❌

```rust
// build.rs ligne 281 — chemin par défaut
write_artifacts(&out, &[], &ZERO_HASH, &ZERO_HASH);
```

Au runtime, `kernel_a_hash_is_zero()` retourne `true`, et `verify_merkle()` échoue immédiatement :

```rust
// forge.rs ligne 291
if kernel_a_hash_is_zero() || elf.text.is_empty() || elf.rodata.is_empty() {
    return Err(ForgeError::MerkleVerifyFailed);
}
```

**Tout `cargo build` sans pipeline de double passe A/B produit un kernel où la résurrection ExoPhoenix est définitivement inopérante**, sans avertissement visible dans la sortie de boot. La feature `EXOPHOENIX_WARN_DEGRADED` qui émettrait un warning `cargo` doit être activée explicitement ; elle est désactivée par défaut.

---

### 4.3 `IPC_MAX_PROCESSES` ≠ `MAX_PROCESSES`

**Fichiers concernés :** `arch/constants.rs`, `ipc/core/constants.rs`

```rust
// arch/constants.rs
pub const MAX_PROCESSES: usize = 32_768;   // capacité du registre processus

// ipc/core/constants.rs
pub const IPC_MAX_PROCESSES: usize = 65_536; // dimensionnement IPC
```

Le sous-système IPC se dimensionne pour **deux fois** plus de processus que la table de processus ne peut en contenir. Aucun `assert!` ne lie ces deux constantes. `arch/constants.rs` vérifie seulement que `MAX_PROCESSES <= 65_536` (borne haute IPC) :

```rust
const _: () = assert!(
    MAX_PROCESSES <= 65_536,
    "MAX_PROCESSES is too large for current SSR/process table assumptions"
);
```

Ce guard empêche de dépasser 65 536 mais n'impose pas l'égalité. Si l'IPC alloue des bitmaps ou des tables indexées par PID sur la base de `IPC_MAX_PROCESSES`, elles seront surdimensionnées de 100 %. Si c'est `MAX_PROCESSES` qui est utilisé pour des lookups IPC, des accès out-of-bounds sont possibles pour les PIDs entre 32 769 et 65 536.

---

## 5. Incohérences structurelles

### 5.1 `assert!` dans des helpers de sécurité critiques

**Fichier concerné :** `security/exocage.rs` (lignes 183, 191, 199, 207)

Les quatre helpers d'accès au `_cold_reserve` du TCB utilisent `assert!()` en mode release, avec un commentaire qui reconnaît le problème :

```rust
// exocage.rs ligne 183
assert!(offset + 8 <= 88,
    "PATCH-P1-DEBUG: TCB _cold_reserve write out of bounds: offset={}",
    offset);
// promis debug_assert → assert (release visible)
```

Un `assert!()` qui échoue provoque un **kernel panic** et arrête la machine. Dans le contexte d'une violation CET (Shadow Stack corrompu), la réaction attendue est d'escalader vers ExoPhoenix, pas de paniquer. La "promesse" de revenir à `debug_assert!` n'a pas été tenue, mais la bonne correction serait une gestion d'erreur explicite avec handoff.

---

### 5.2 Checklist G9 de `forge.rs` avec marqueurs `[ADAPT]`

**Fichier concerné :** `exophoenix/forge.rs` (lignes 458, 611, 626, 668)

Plusieurs étapes de la checklist post-reconstruction obligatoire (G9) contiennent des marqueurs `[ADAPT]` indiquant que le code doit encore être connecté aux vraies APIs :

**`drain_dma_queues()` (ligne 458) :**

```rust
fn drain_dma_queues(bus: u8, device: u8, func: u8) {
    // [ADAPT] : utiliser l'API DMA existante du codebase si disponible
    // Fallback : busy-wait 200µs (timeout drain par device class)
    let _ = wait_apic_timeout_us(200);
    let _ = (bus, device, func);
}
```

L'API DMA existe dans `memory/dma/`. Le busy-wait de 200 µs est arbitraire et ne garantit pas que les DMA en vol sont terminées sur des devices lents.

**`checklist_idt_has_exophoenix_vectors()` (ligne 668) :**

La vérification lit l'IDTR du **CPU courant** (Kernel B) via `SIDT`, alors qu'elle devrait lire l'IDT de **Kernel A** via un accès physique direct. Dans le contexte de la résurrection, Kernel B et Kernel A partagent physiquement la même IDT au moment du check, ce qui rend la vérification triviale et sans valeur de sécurité.

```rust
// Lit l'IDTR du CPU courant (B), pas l'IDT physique de A
unsafe {
    core::arch::asm!("sidt [{ptr}]", ptr = in(reg) &mut idtr, ...);
}
```

---

### 5.3 APs SMP accèdent au scheduler avant `SECURITY_READY`

**Fichier concerné :** `arch/x86_64/smp/init.rs`

La séquence d'entrée AP (`ap_entry`) appelle `publish_current_boot_idle()` et `scheduler::init_ap()` **avant** d'atteindre le spin-wait sur `SECURITY_READY` (étape 8b) :

```rust
// ap_entry — ordre d'exécution
// 6b. publish_current_boot_idle()   ← accès scheduler (pas de dépendance IPC directe)
// 6c. scheduler::init_ap()          ← init locale scheduler
// 7.  apply_mitigations_ap()
// 8b. while !security::is_security_ready() { spin }  ← trop tard
```

Les appels 6b et 6c n'accèdent pas à l'IPC, mais ils modifient des structures scheduler partagées. Si `security_init()` (qui court sur le BSP) modifie des tables partagées entre scheduler et security dans la Phase 5, une fenêtre de concurrence existe. Le commentaire documente explicitement le risque CVE-EXO-001 mais la position du guard ne couvre pas les appels antérieurs.

---

### 5.4 `throttle_or_kill` : faux positif par `saturating_mul`

**Fichier concerné :** `security/exokairos.rs` (ligne 271)

```rust
fn throttle_or_kill(used: u64, budget: u64) -> Result<(), CapError> {
    let used_pct = used.saturating_mul(100);          // peut saturer à u64::MAX
    if used_pct >= budget.saturating_mul(KAIROS_KILL_PCT) {  // budget × 200
        return Err(CapError::KillThresholdExceeded);
    }
    ...
}
```

Si `used` dépasse `u64::MAX / 100` (~1.84 × 10¹⁷, soit ~5 840 années en nanosecondes), `used_pct` sature à `u64::MAX`. La comparaison avec `budget.saturating_mul(200)` retourne `true` quel que soit le budget réel, déclenchant un faux `KillThresholdExceeded`. Les budgets sont exprimés en nanosecondes sur une fenêtre de 1 seconde (`KAIROS_WINDOW_NS = 1_000_000_000`), ce qui rend ce cas théoriquement hors portée en opération normale — mais il n'y a aucune borne (`assert!` ou saturation intentionnelle) sur les valeurs entrantes.

---

## 6. Incohérences mineures

### 6.1 `pub use arch::x86_64` sans garde `#[cfg]`

**Fichier concerné :** `lib.rs` (ligne 90)

```rust
// lib.rs ligne 90 — PAS de #[cfg(target_arch = "x86_64")]
pub use arch::x86_64::{
    arch_info,
    boot::early_init::arch_boot_init,
    halt_cpu,
    memory_barrier,
    KERNEL_BASE,
    PAGE_SIZE,
};

// ligne 102 — correctement gardé
#[cfg(target_arch = "x86_64")]
pub use arch::ArchInfo;
```

Le re-export de la ligne 90 est la seule occurrence sans garde dans ce bloc. Le `compile_error!` de la ligne 30 empêche la compilation sur AArch64, mais c'est un filet de sécurité externe ; retirer accidentellement ce guard laisserait une erreur de résolution de symbole cryptique (`arch::x86_64` inexistant sur AArch64) plutôt qu'un message clair.

---

### 6.2 `arch::time::read_ticks()` ignore le vrai timer AArch64

**Fichiers concernés :** `arch/time.rs`, `arch/aarch64/mod.rs`

```rust
// arch/time.rs
pub fn read_ticks() -> u64 {
    #[cfg(target_arch = "x86_64")]
    { crate::arch::x86_64::cpu::tsc::read_tsc() }

    #[cfg(not(target_arch = "x86_64"))]
    { 0u64 }   // ← retourne zéro
}
```

Or `arch/aarch64/mod.rs` implémente déjà un vrai compteur via `CNTVCT_EL0` :

```rust
// arch/aarch64/mod.rs
pub fn read_tsc() -> u64 {
    unsafe { core::arch::asm!("mrs {}, cntvct_el0", ...) }
}
```

Les deux modules coexistent sans se référencer. `arch::time::read_ticks()` retourne `0` sur toute cible non-x86_64 au lieu d'appeler `aarch64::read_tsc()`.

---

### 6.3 Allocation statique `ProcessRegistry` de 512 Ko au boot

**Fichier concerné :** `process/core/registry.rs`

```rust
// ProcessInitParams::default()
max_pids: 32768

// ProcessRegistry::init()
let layout = Layout::array::<RegistrySlot>(capacity)...;
let ptr = alloc_zeroed(layout) as *mut RegistrySlot;
```

`RegistrySlot` = `AtomicPtr<PCB>` (8 octets) + `AtomicU32` (4 octets) + padding (4 octets) = **16 octets**. L'allocation statique au boot est donc **32 768 × 16 = 524 288 octets (512 Ko)** d'un seul appel à `alloc_zeroed`. La cible QEMU de référence déclare `-m 256M`. Il n'existe aucune vérification de la mémoire disponible avant cette allocation, et aucun mécanisme de dimensionnement dynamique.

---

### 6.4 21 occurrences de `ENOSYS` dans `syscall/table.rs`

**Fichier concerné :** `syscall/table.rs`

```bash
grep -c "ENOSYS" kernel/src/syscall/table.rs
# 21
```

Vingt-et-un appels système retournent systématiquement `ENOSYS`. Certains correspondent à des syscalls délibérément non implémentés (attendu), mais d'autres concernent des fonctionnalités documentées comme actives (ex. variantes de `wait`, `getdents`, `fcntl`). L'absence de distinction entre "non implémenté par conception" et "stub temporaire" rend le suivi de la complétude POSIX difficile.

---

## 7. Tableau récapitulatif

| # | Sévérité | Module | Description |
|---|---|---|---|
| 1 | 🔴 Critique | `exophoenix/stage0.rs` | `stage0_init_all_steps()` jamais appelé → `IOMMU_BLOCKED_DOMAIN_ID = 0` au handoff |
| 2 | 🔴 Critique | `kernel/build.rs` | Contrat ExoPhoenix nul par défaut (hashes à zéro sans double passe A/B) |
| 3 | 🔴 Critique | `arch/constants.rs` vs `ipc/core/constants.rs` | `MAX_PROCESSES` (32 768) ≠ `IPC_MAX_PROCESSES` (65 536) |
| 4 | 🟠 Structurel | `security/exocage.rs` | `assert!()` release dans hot path sécurité au lieu d'escalade ExoPhoenix |
| 5 | 🟠 Structurel | `exophoenix/forge.rs` | `[ADAPT]` : `drain_dma_queues` bidon (200 µs fixe) + IDT lue depuis Kernel B, pas A |
| 6 | 🟠 Structurel | `arch/x86_64/smp/init.rs` | APs accèdent au scheduler avant `SECURITY_READY` (fenêtre de concurrence) |
| 7 | 🟠 Structurel | `security/exokairos.rs` | `saturating_mul(100)` → faux kill si `used > u64::MAX / 100` |
| 8 | 🟡 Mineur | `lib.rs` | `pub use arch::x86_64` sans `#[cfg(target_arch = "x86_64")]` |
| 9 | 🟡 Mineur | `arch/time.rs` | Fallback non-x86_64 retourne `0` au lieu d'appeler `aarch64::read_tsc()` |
| 10 | 🟡 Mineur | `process/core/registry.rs` | Allocation statique 512 Ko au boot sans vérification mémoire disponible |
| 11 | 🟡 Mineur | `syscall/table.rs` | 21 occurrences `ENOSYS` sans distinction conception/stub temporaire |

---

## 8. Recommandations

### Priorité 1 — Bloquer la mise en production

**Incohérence #1 — Appel manquant à `stage0_init_all_steps()`**

Ajouter l'appel dans `kernel_init()`, après `security_init()` (Phase 5) mais avant `ipc_init()` (Phase 6), car `stage0` nécessite le sous-système de sécurité pour les capabilities IOMMU :

```rust
// lib.rs — kernel_init(), entre Phase 5 et Phase 6
unsafe {
    crate::exophoenix::stage0::stage0_init_all_steps();
}
kdb(b'B'); // stage0 ExoPhoenix ready
```

**Incohérence #2 — Contrat ExoPhoenix nul par défaut**

Activer `EXOPHOENIX_WARN_DEGRADED=1` par défaut dans le `Makefile`, ou mieux, ajouter dans `kernel_init()` un log de boot explicite si `kernel_a_hash_is_zero()` est vrai :

```rust
if crate::exophoenix::forge::kernel_a_hash_is_zero() {
    crate::arch::x86_64::boot_display::stage_warn(
        "EXOPHOENIX: contrat Kernel A absent — résurrection inopérante"
    );
}
```

**Incohérence #3 — Désynchronisation `MAX_PROCESSES` / `IPC_MAX_PROCESSES`**

Lier les deux constantes par une assertion compile-time dans `ipc/core/constants.rs` :

```rust
const _: () = assert!(
    IPC_MAX_PROCESSES == crate::arch::constants::MAX_PROCESSES,
    "IPC_MAX_PROCESSES doit être égal à MAX_PROCESSES (arch/constants.rs)"
);
```

### Priorité 2 — Corriger avant v0.2.0

**Incohérence #4 — `assert!` dans ExoCage**

Remplacer les quatre `assert!()` par une gestion d'erreur explicite qui logue via port 0xE9 et déclenche le handoff ExoPhoenix :

```rust
unsafe fn tcb_write_cold_u64(tcb: &mut ThreadControlBlock, offset: usize, val: u64) {
    if offset + 8 > 88 {
        // SAFETY: context d'une violation CET — escalade immédiate
        crate::exophoenix::handoff::trigger_emergency_handoff();
    }
    ...
}
```

**Incohérence #5 — Checklist G9**

Connecter `drain_dma_queues()` à `crate::memory::dma::drain_pending_for_device()` au lieu du busy-wait. Pour `checklist_idt_has_exophoenix_vectors()`, lire l'IDTR de Kernel A via l'accès physique décrit dans le commentaire `[ADAPT]`.

**Incohérence #6 — Race APs / `SECURITY_READY`**

Déplacer le spin-wait `SECURITY_READY` avant les appels à `publish_current_boot_idle()` et `scheduler::init_ap()`, ou documenter formellement que ces fonctions sont indépendantes de `security_init()` et ne peuvent pas créer de race.

### Priorité 3 — Dette technique

**Incohérence #7 — ExoKairos `saturating_mul`**

Ajouter une borne défensive sur les valeurs entrantes :

```rust
fn throttle_or_kill(used: u64, budget: u64) -> Result<(), CapError> {
    debug_assert!(used <= KAIROS_WINDOW_NS * 2, "used budget hors borne");
    debug_assert!(budget <= KAIROS_WINDOW_NS * 2, "budget hors borne");
    ...
}
```

**Incohérences #8, #9 — Arch guards**

Ajouter `#[cfg(target_arch = "x86_64")]` sur le re-export ligne 90 de `lib.rs`, et connecter `arch/time.rs` à `aarch64::read_tsc()` :

```rust
// arch/time.rs
#[cfg(not(target_arch = "x86_64"))]
{ crate::arch::aarch64::read_tsc() }
```

**Incohérences #10, #11**

Documenter explicitement la limite mémoire de 512 Ko dans `ProcessRegistry::init()` avec un commentaire de justification. Créer une macro `syscall_stub!(NOM)` qui distingue les stubs intentionnels des implémentations manquantes, et auditer les 21 occurrences pour classer chacune.

---

*Rapport généré par analyse statique du dépôt — aucune exécution, aucun fuzzing.*  
*Les numéros de ligne référencés correspondent au commit cloné le 2026-06-05.*
