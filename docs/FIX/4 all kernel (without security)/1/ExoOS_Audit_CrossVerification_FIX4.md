# ExoOS — Audit Croisé & Rapport de Corrections
## FIX-4 : Vérification des rapports externes + Nouveaux bugs kernel
**Commit analysé :** `74c3659e`  
**Date :** 20 avril 2026  
**Méthode :** Lecture directe du code source + cross-référence docs/recast + vérification de chaque
allégation des rapports externes (Documents 1–5) contre le code réel du dépôt.

---

## Table des matières

1. [Vérification des allégations des rapports externes](#1-vérification-des-allégations-des-rapports-externes)
2. [Bugs confirmés issus des rapports externes](#2-bugs-confirmés-issus-des-rapports-externes)
3. [Bugs nouveaux découverts par cet audit (non couverts par FIX-4 ni les rapports externes)](#3-bugs-nouveaux-découverts-par-cet-audit)
4. [Plan de correction consolidé](#4-plan-de-correction-consolidé)

---

## 1. Vérification des allégations des rapports externes

Cette section tranche chaque allégation des documents D1–D5 par une vérification directe
dans les fichiers sources. Les allégations **fausses** (déjà corrigées ou jamais vraies)
sont archivées pour clôture. Les allégations **vraies** passent en Section 2.

### 1.1 Allégations FAUSSES — déjà corrigées dans le commit courant

| Allégation | Rapport | Verdict | Preuve dans le code |
|------------|---------|---------|---------------------|
| `SSR_MAX_CORES_LAYOUT` divergence crate vs kernel | D1, D2, D3 | ❌ FAUX | `libs/exo-phoenix-ssr/src/lib.rs:36` : `pub const SSR_MAX_CORES_LAYOUT: usize = 256;` ; `kernel/src/exophoenix/ssr.rs:10` importe cette constante — unifié. |
| `security::init()` non câblé au boot | D1, D2, D3 | ❌ FAUX | `kernel/src/lib.rs:230` : `crate::security::security_init(...)` appelé dans la séquence de boot. |
| `init_syscall()` uniquement sur BSP | D1, D2, D3 | ❌ FAUX | `kernel/src/arch/x86_64/smp/init.rs:127` : `super::super::syscall::init_syscall()` appelé pour chaque AP. |
| `gs:[0x20]` non mis à jour au context switch | D1, D2, D3 | ❌ FAUX | `kernel/src/scheduler/core/switch.rs:272` : `percpu::set_current_tcb(next as *mut ThreadControlBlock)` présent. |
| `check_sys_admin_capability` retourne `true` en dur | D1, D2, D3 | ❌ FAUX | `kernel/src/drivers/device_claims.rs:69–78` : vérifie `PROCESS_REGISTRY.find_by_pid(pid)?.is_root()`. |
| `md_mmio_whitelist_contains` retourne `true` en dur | D1, D2, D3 | ❌ FAUX | `kernel/src/drivers/device_claims.rs:80–94` : parcourt `MEMORY_MAP[..MEMORY_REGION_COUNT]` et filtre sur `MemoryRegionType::Reserved`. |
| `static mut IDT` sans protection SMP | D5 | ❌ FAUX | `idt.rs:169` commente « L'IDT est read-only après init » ; seul `init_idt()` écrit, appelé une fois par le BSP ; `IDT_INITIALIZED: AtomicBool` gère la double-init ; `get_idt_entry()` ne fait que lire. |
| `MAX_CPUS` défini à 3 valeurs différentes (256/128/512) | D5 | ❌ FAUX | Toutes les définitions de `MAX_CPUS` dans le kernel sont à **256** (`cpu/topology.rs:14`, `percpu.rs:24`, `ktime.rs:299`, `preempt.rs:31`, `smp/topology.rs:14`, `memory/core/constants.rs:113`). Les constantes à 512 ont des noms distincts (`STOLEN_TIME_MAX_CPUS`, `RECLAIM_MAX_CPUS`). |
| `TSS.RSP0` non mis à jour au context switch | D5 | ❌ FAUX | `kernel/src/scheduler/core/switch.rs:301` : `tss::update_rsp0(next.current_cpu().0 as usize, next.kstack_ptr)` présent. |
| Lazy FPU / `CR0.TS` non géré | D5 | ❌ FAUX | `switch.rs:176–291` : XSAVE si `fpu_loaded()`, puis `fpu::lazy::cr0_set_ts()` + `set_fpu_loaded(false)`. Handler `#NM` (`fpu::lazy`) gère le restore. |
| TOCTOU dans `sys_pci_claim` | D1, D2, D5 | ❌ FAUX | `device_claims.rs:6` : « TOCTOU Protection (CORR-32) : Le lock d'écriture est pris *avant* toute vérification » ; `device_claims.rs:133–136` : vérifications faites **sous** `DEVICE_CLAIMS.write()`. |
| `verify_cap_token` non constant-time | D1, D2, D5 | ❌ FAUX | `security/crypto/blake3.rs:29` : `use subtle::ConstantTimeEq` ; `security/capability/verify.rs:50` : variante `Denied` retournée systématiquement (timing-safe). |
| Stack canaries absentes | D5 | ❌ FAUX | `memory/integrity/canary.rs` : module complet avec `STACK_CANARY_INITIAL = 0xDEAD_BEEF_CAFE_BABE`, table par CPU, handler violation. |
| `static mut EXOFS_SUPERBLOCK` race condition | D5 | ❌ FAUX (dead code) | `fs/exofs/mod.rs:52` : `#[allow(dead_code)]` explicite ; grep confirme zéro lecture de cette variable dans tout le codebase. C'est du dead code, pas une race. |
| `CapToken` replay attack (pas de nonce/TTL) | D5 | Hors scope FIX-4 | Documenté dans ExoShield Phase 5 ; architecturalement prévu, pas encore implémenté. Non bloquant pour FIX-4. |

### 1.2 Allégations VRAIES confirmées par code

Quatre allégations sont confirmées par lecture directe du code source et passent
en Section 2 pour correction.

---

## 2. Bugs confirmés issus des rapports externes

---

### EXT-01 — 16 assertions de taille `SIZE_ASSERT_DISABLED` dans ExoFS
**Rapports :** D3 (ERREUR-CRITIQUE-03), D5  
**Gravité :** P1 — Corruption ABI silencieuse potentielle  
**Fichiers concernés (13 fichiers, 16 assertions) :**

```
kernel/src/fs/exofs/audit/audit_entry.rs          → AuditEntry == AUDIT_ENTRY_SIZE
kernel/src/fs/exofs/core/epoch_id.rs              → EpochCommitSummary == 32
kernel/src/fs/exofs/export/exoar_format.rs        → ExoarHeader == 128
                                                    ExoarEntryHeader == 96
                                                    ExoarFooter == 32
kernel/src/fs/exofs/export/stream_import.rs       → ImportEntryHeader == 52
kernel/src/fs/exofs/posix_bridge/mmap.rs          → MmapEntry == 48
kernel/src/fs/exofs/posix_bridge/vfs_compat.rs    → VfsDirent == 271
kernel/src/fs/exofs/syscall/object_create.rs      → CreateResult == 88
kernel/src/fs/exofs/syscall/object_open.rs        → OpenResult == 72
kernel/src/fs/exofs/syscall/object_stat.rs        → ObjectStat == 176
kernel/src/fs/exofs/syscall/path_resolve.rs       → PathResolveResult == 104
kernel/src/fs/exofs/syscall/relation_create.rs    → RelationCreateArgs == 104
kernel/src/fs/exofs/syscall/relation_query.rs     → RelationQueryArgs == 56
kernel/src/fs/exofs/syscall/snapshot_mount.rs     → SnapshotMountResult == 64
                                                    MountEntry == 64
```

**Forme actuelle (toutes les occurrences) :**
```rust
// SIZE_ASSERT_DISABLED: const _: () = assert!(core::mem::size_of::<VfsDirent>() == 271);
```

**Problème :** Ces assertions de taille sont les gardiennes de l'ABI entre le kernel et
userspace. Si le compilateur ou une modification de struct change silencieusement la taille
d'un type (padding, alignement, ajout de champ), aucune erreur de compilation n'est levée.
Les conséquences sont : lecture de champ au mauvais offset dans `readdir()`,
désérialisation corrompue dans le protocole ExoAR, `stat()` retournant des valeurs fausses.  

Ces assertions ont été commentées avec le marqueur `SIZE_ASSERT_DISABLED` — probablement
pour contourner un problème de compilation temporaire — sans jamais être réactivées.

**Correction :** Réactiver **toutes les 16 assertions** en retirant le préfixe commentaire.
Si une assertion échoue à la compilation, c'est un vrai problème ABI à corriger en priorité,
pas à masquer.

```rust
// AVANT (incorrect) :
// SIZE_ASSERT_DISABLED: const _: () = assert!(core::mem::size_of::<VfsDirent>() == 271);

// APRÈS (correct) :
const _: () = assert!(
    core::mem::size_of::<VfsDirent>() == 271,
    "VfsDirent ABI size changed — vérifier tous les appels readdir() userspace"
);
const _: () = assert!(
    core::mem::align_of::<VfsDirent>() == 8,
    "VfsDirent alignment changed"
);
```

Appliquer le même schéma pour les 15 autres assertions. Ajouter un message d'erreur
explicite à chacune pour guider le développeur si elle échoue.

**Impact si non corrigé :** Toute modification de struct ExoFS (ajout d'un champ, changement
d'alignement) passera silencieusement la compilation et introduira une corruption ABI
non détectée jusqu'à l'exécution — potentiellement en production.

---

### EXT-02 — `OpenFdTable` : `Vec<VfsFd>` non borné en contexte kernel
**Rapports :** D3 (ERREUR-CRITIQUE-04), D5  
**Gravité :** P1  
**Fichier :** `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs:134–140`

**Code actuel :**
```rust
struct OpenFdTable {
    fds:      UnsafeCell<Vec<VfsFd>>,  // ← allocation heap dynamique
    spinlock: AtomicU64,
    next_fd:  AtomicU64,
}
```

**Problème en deux niveaux :**

**Niveau 1 — Absence de borne :** `Vec<VfsFd>` peut grandir sans limite.
Un processus Ring3 malveillant peut appeler `open()` en boucle jusqu'à l'OOM kernel.
Contrairement à un système POSIX qui retourne `EMFILE`/`ENFILE`, le kernel peut paniquer
si l'allocateur global manque de mémoire.

**Niveau 2 — Allocation heap depuis un spinlock :** `open_fd()` tient `spinlock`
(un spin actif) pendant que `Vec::push()` peut déclencher une réallocation heap.
Si l'allocateur kernel est lui-même protégé par un verrou (fréquent dans les SLABs),
cela crée un risque d'inversion de verrou ou de latence indéterminée sous IRQ.

**Correction :**
```rust
// kernel/src/fs/exofs/posix_bridge/vfs_compat.rs

/// Nombre maximum de descripteurs ouverts par processus (POSIX : OPEN_MAX = 1024).
/// Fixe la taille statique de la table pour éviter toute allocation dynamique.
pub const VFS_OPEN_MAX: usize = 1024;

struct OpenFdTable {
    // Tableau fixe — ZERO allocation heap, taille déterministe
    fds:      UnsafeCell<[Option<VfsFd>; VFS_OPEN_MAX]>,
    fd_count: AtomicUsize,
    spinlock: AtomicU64,
    next_fd:  AtomicU64,
}

// Assertion compile-time : la table FD doit tenir en moins de 64KB
const _: () = assert!(
    core::mem::size_of::<[Option<VfsFd>; VFS_OPEN_MAX]>() <= 65536,
    "FD table trop grande — réduire VFS_OPEN_MAX"
);

impl OpenFdTable {
    const fn new() -> Self {
        Self {
            fds:      UnsafeCell::new([None; VFS_OPEN_MAX]),
            fd_count: AtomicUsize::new(0),
            spinlock: AtomicU64::new(0),
            next_fd:  AtomicU64::new(3),
        }
    }

    fn open_fd(&self, ino: ObjectIno, flags: u32, pid: u32) -> ExofsResult<u64> {
        self.lock_acquire();
        let fds = unsafe { &mut *self.fds.get() };

        // Vérification EMFILE avant toute allocation
        if self.fd_count.load(Ordering::Relaxed) >= VFS_OPEN_MAX {
            self.lock_release();
            return Err(ExofsError::TooManyOpenFiles); // EMFILE
        }
        // ...trouver slot libre dans fds[..], l'écrire, incrémenter fd_count...
    }
}
```

---

### EXT-03 — `run_queue()` : `debug_assert!` désactivé en release, lecture MaybeUninit non protégée
**Rapports :** D5 (ERR-004)  
**Gravité :** P2  
**Fichier :** `kernel/src/scheduler/core/runqueue.rs:669–672`

**Code actuel :**
```rust
pub unsafe fn run_queue(cpu: CpuId) -> &'static mut PerCpuRunQueue {
    // SAFETY: init_percpu() garantit que toutes les run queues sont initialisées.
    debug_assert!((cpu.0 as usize) < MAX_CPUS, "CPU id hors limites");
    // ↑ debug_assert! = NO-OP en --release → vérification absente en production
    PER_CPU_RQ[cpu.0 as usize].assume_init_mut()
}
```

**Problème :** `debug_assert!` est compilé uniquement en mode debug
(`cfg(debug_assertions)`). En build release, si `run_queue()` est appelé avec un
`cpu.0 >= MAX_CPUS` ou avant `init_percpu()`, le CPU lit de la mémoire non initialisée
(`MaybeUninit::uninit()` = zéro-init sur BSS, mais sémantiquement UB).

**Correction :** Remplacer `debug_assert!` par `assert!` pour la vérification de bornes
(coût négligeable — appelé rarement hors chemin chaud) et ajouter un `AtomicBool`
de tracking d'initialisation :

```rust
// runqueue.rs

static RUNQUEUE_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub fn init_percpu(nr_cpus: usize) {
    unsafe {
        for i in 0..nr_cpus.min(MAX_CPUS) {
            PER_CPU_RQ[i].write(PerCpuRunQueue::new(CpuId(i as u32)));
        }
    }
    RUNQUEUE_INITIALIZED.store(true, Ordering::Release);
}

pub unsafe fn run_queue(cpu: CpuId) -> &'static mut PerCpuRunQueue {
    // Vérification active en release (bornes + initialisation)
    assert!(
        (cpu.0 as usize) < MAX_CPUS,
        "run_queue: cpu {} hors limites (MAX_CPUS={})", cpu.0, MAX_CPUS
    );
    assert!(
        RUNQUEUE_INITIALIZED.load(Ordering::Acquire),
        "run_queue: appelé avant init_percpu() — séquence de boot incorrecte"
    );
    // SAFETY: init_percpu() confirmé appelé ci-dessus.
    PER_CPU_RQ[cpu.0 as usize].assume_init_mut()
}
```

---

### EXT-04 — `static mut EXOFS_SUPERBLOCK` : dead code non nettoyé
**Rapports :** D5 (ERR-002)  
**Gravité :** P2 (maintenance)  
**Fichier :** `kernel/src/fs/exofs/mod.rs:52`

**Code actuel :**
```rust
/// Référence globale au superblock actif (protégée par SpinLock dans SuperblockInMemory)
#[allow(dead_code)]
static mut EXOFS_SUPERBLOCK: Option<Arc<SuperblockInMemory>> = None;
```

**Verdict :** La race condition alléguée dans D5 est **inexistante** car cette variable
n'est jamais lue ni écrite ailleurs dans le codebase (grep confirme zéro accès en dehors
de la déclaration). Cependant, sa présence avec `#[allow(dead_code)]` et `static mut`
constitue une dette technique :

- Le `static mut` aurait pu être transformé en race condition si un futur développeur
  avait tenté de l'utiliser sans voir qu'elle est orpheline.
- Le commentaire `"protégée par SpinLock"` est trompeur puisque aucun lock n'encadre
  l'accès à l'`Option` elle-même.

**Correction :** Supprimer la déclaration ou la remplacer par une implémentation correcte
si un accès centralisé au superblock est prévu :

```rust
// OPTION A — Suppression (si accès via superblock::get() suffit)
// Retirer les lignes 51-53 de fs/exofs/mod.rs

// OPTION B — Remplacement correct si une référence globale est nécessaire
use spin::Once;
static EXOFS_SUPERBLOCK: Once<Arc<SuperblockInMemory>> = Once::new();

pub fn get_superblock() -> Option<&'static Arc<SuperblockInMemory>> {
    EXOFS_SUPERBLOCK.get()
}
```

---

## 3. Bugs nouveaux découverts par cet audit

Ces bugs sont absents des rapports externes D1–D5 ET des documents FIX-4 précédents.
Ils ont été trouvés par lecture directe du code source.

---

### NEW-P0-06 — `clone_pt()` ne retire pas `FLAG_WRITABLE` : CoW non implémenté
**Gravité :** P0 — Critique  
**Fichier :** `kernel/src/memory/virtual/address_space/fork_impl.rs`

Le fichier se décrit comme « implémentant le Copy-on-Write » dans son en-tête, mais
`clone_pt()` copie chaque PTE **telle quelle** sans modifier un seul bit :

```rust
// CODE ACTUEL — FAUX (aucun CoW réel)
unsafe fn clone_pt(src_pt_phys: PhysAddr, dst_pt_phys: PhysAddr) {
    let src_pt = phys_to_table_ref(src_pt_phys);
    let dst_pt = phys_to_table_mut(dst_pt_phys);
    for l1_idx in 0..512 {
        let src_entry = src_pt[l1_idx];
        if src_entry.is_present() {
            dst_pt[l1_idx] = src_entry; // FLAG_WRITABLE intact dans parent ET fils
        }
    }
}
```

`FLAG_WRITABLE (bit 1)` reste à 1 dans les PTEs du **parent** et du **fils**.
`FLAG_COW (bit 9)` n'est jamais posé. Résultat : les deux espaces d'adressage peuvent
écrire simultanément sur les mêmes frames physiques → corruption mémoire silencieuse
garantie après tout `fork()`.

De plus, `repoint_table_entry()` utilisé dans `clone_pd()` et `clone_pdpt()`
applique un masque qui **préserve `FLAG_WRITABLE`** dans les entrées intermédiaires,
propageant le problème jusqu'aux huge pages.

**Correction complète :**

```rust
// kernel/src/memory/virtual/address_space/fork_impl.rs

use crate::memory::virt::page_table::x86_64::PageTableEntry as PTE;

unsafe fn clone_pt(src_pt_phys: PhysAddr, dst_pt_phys: PhysAddr) {
    let src_pt = phys_to_table_ref(src_pt_phys);
    let dst_pt = phys_to_table_mut(dst_pt_phys);

    for l1_idx in 0..512 {
        let src_entry = src_pt[l1_idx];
        if !src_entry.is_present() { continue; }

        if src_entry.is_writable() {
            // Créer l'entrée CoW : READ_ONLY + FLAG_COW dans parent ET fils
            let cow_entry = PTE::from_raw(
                (src_entry.raw() & !PTE::FLAG_WRITABLE) | PTE::FLAG_COW
            );
            // Parent perd WRITABLE — TLB shootdown obligatoire après clone_cow()
            src_pt[l1_idx] = cow_entry;
            dst_pt[l1_idx] = cow_entry;
            // Incrémenter le refcount de la frame pour éviter double-free
            crate::memory::physical::frame::ref_count::inc_refcount(
                src_entry.phys_addr()
            );
        } else {
            // Page déjà read-only (ex: .text) → partage direct sans refcount
            dst_pt[l1_idx] = src_entry;
        }
    }
}

// AUSSI : huge pages dans clone_pdpt() et clone_pd()
// Ajouter la même logique pour les entries is_huge() :
if src_entry.is_huge() {
    if src_entry.is_writable() {
        let cow_huge = PTE::from_raw(
            (src_entry.raw() & !PTE::FLAG_WRITABLE) | PTE::FLAG_COW
        );
        src_pdpt[l3_idx] = cow_huge;
        dst_pdpt[l3_idx] = cow_huge;
        // NOTE : order = 9 pour 2MB, order = 18 pour 1GB
        crate::memory::physical::frame::ref_count::inc_refcount(src_entry.phys_addr());
    } else {
        dst_pdpt[l3_idx] = src_entry;
    }
    continue;
}
```

Après la correction, `flush_tlb_after_fork()` doit envoyer un IPI à tous les CPUs
exécutant le parent (TLB shootdown SMP) car des PTEs du parent ont été modifiées
en read-only. La version actuelle recharge seulement le CR3 local.

---

### NEW-P0-07 — `ElfLoader` retourne `cr3 = 0x1000` hardcodé → triple fault
**Gravité :** P0 — Critique  
**Fichier :** `kernel/src/fs/elf_loader_impl.rs:59`

```rust
// CODE ACTUEL — FAUX
let cr3 = 0x1000u64; // Placeholder CR3 — doit être alloué réellement

Ok(ElfLoadResult {
    entry_point: 0x0000_7f00_0000_1000u64, // adresse arbitraire non mappée
    cr3,           // ← page physique 1 = zone BIOS/IVT sous QEMU
    addr_space_ptr: cr3 as usize,
    ...
})
```

`0x1000` est la page physique 4096 — zone BIOS / Real-Mode IVT sous QEMU.
Quand `do_execve()` charge ce CR3, le CPU interprète les 8 octets à `phys[0x1000]`
comme l'entrée PML4[0] → structure invalide → `#PF` → `#DF` → triple fault → reset CPU.

Le cas n'est déclenché que pour les chemins contenant `"init_server"`.
Tous les autres chemins retournent `ElfLoadError::NotFound`.

**Correction :** Implémenter le chargement ELF réel avec allocation d'un vrai PML4 :

```rust
impl ElfLoader for ExoFsElfLoader {
    fn load_elf(&self, path: &str, _argv: &[&str], _envp: &[&str], _cr3_in: u64)
        -> Result<ElfLoadResult, ElfLoadError>
    {
        // 1. Résoudre dans ExoFS
        let blob_id = crate::fs::exofs::path::resolve(path)
            .map_err(|_| ElfLoadError::NotFound)?;

        // 2. Lire l'en-tête ELF (64 bytes)
        let mut hdr = [0u8; 64];
        crate::fs::exofs::object::read_bytes(blob_id, 0, &mut hdr)
            .map_err(|_| ElfLoadError::IoError)?;

        // 3. Valider le magic ELF + arch x86_64
        if &hdr[0..4] != b"\x7FELF" { return Err(ElfLoadError::InvalidMagic); }
        if hdr[4] != 2 || hdr[5] != 1 { return Err(ElfLoadError::UnsupportedArch); }
        let e_machine = u16::from_le_bytes([hdr[18], hdr[19]]);
        if e_machine != 0x3E { return Err(ElfLoadError::UnsupportedArch); }

        let e_entry = u64::from_le_bytes(hdr[24..32].try_into().unwrap());
        let e_phoff = u64::from_le_bytes(hdr[32..40].try_into().unwrap());
        let e_phnum = u16::from_le_bytes([hdr[56], hdr[57]]) as usize;

        // 4. Allouer un nouvel espace d'adressage (PML4 réel)
        let child_pml4 = crate::memory::physical::allocator::buddy::alloc_pages(
            0, crate::memory::AllocFlags::ZEROED
        ).map_err(|_| ElfLoadError::OutOfMemory)?;
        let new_cr3 = child_pml4.start_address().as_u64();

        // Copier les entrées kernel PML4[256..512] depuis le CR3 courant
        // (mapping kernel identique pour tous les processus)
        let current_cr3: u64;
        unsafe { core::arch::asm!("mov {}, cr3", out(reg) current_cr3); }
        unsafe {
            use crate::memory::virt::page_table::x86_64::{phys_to_table_ref, phys_to_table_mut};
            use crate::memory::core::PhysAddr;
            let src = phys_to_table_ref(PhysAddr::new(current_cr3));
            let dst = phys_to_table_mut(PhysAddr::new(new_cr3));
            for i in 256..512 { dst[i] = src[i]; }
        }

        // 5. Charger les segments PT_LOAD
        const PHENT: usize = 56;
        let mut brk_end: u64 = 0;
        for i in 0..e_phnum {
            let mut phdr = [0u8; 56];
            crate::fs::exofs::object::read_bytes(blob_id, (e_phoff as usize) + i * PHENT, &mut phdr)
                .map_err(|_| ElfLoadError::IoError)?;
            let p_type   = u32::from_le_bytes(phdr[0..4].try_into().unwrap());
            if p_type != 1 { continue; } // PT_LOAD only
            let p_flags  = u32::from_le_bytes(phdr[4..8].try_into().unwrap());
            let p_offset = u64::from_le_bytes(phdr[8..16].try_into().unwrap());
            let p_vaddr  = u64::from_le_bytes(phdr[16..24].try_into().unwrap());
            let p_filesz = u64::from_le_bytes(phdr[32..40].try_into().unwrap());
            let p_memsz  = u64::from_le_bytes(phdr[40..48].try_into().unwrap());

            // Mapper les pages dans le nouvel espace d'adressage
            map_elf_segment(new_cr3, blob_id, p_vaddr, p_filesz, p_memsz, p_offset, p_flags)
                .map_err(|_| ElfLoadError::OutOfMemory)?;

            let seg_end = p_vaddr + p_memsz;
            if seg_end > brk_end { brk_end = seg_end; }
        }

        // 6. Allouer la pile (8 pages = 32 KiB)
        const STACK_SIZE:  usize = 8 * crate::memory::core::PAGE_SIZE;
        const STACK_TOP:   u64   = 0x0000_7FFF_FFFF_0000;
        let stack_base = STACK_TOP - STACK_SIZE as u64;
        map_anon_region(new_cr3, stack_base, STACK_SIZE, PageFlags::USER_RW)
            .map_err(|_| ElfLoadError::OutOfMemory)?;
        let initial_rsp = (STACK_TOP - 8) & !0xF; // aligné 16B

        let brk_start = (brk_end + crate::memory::core::PAGE_SIZE as u64 - 1)
            & !(crate::memory::core::PAGE_SIZE as u64 - 1);

        Ok(ElfLoadResult {
            entry_point:       e_entry,
            initial_stack_top: initial_rsp,
            tls_base:          0,
            tls_size:          0,
            brk_start,
            cr3:               new_cr3,
            addr_space_ptr:    new_cr3 as usize,
            signal_tcb_vaddr:  0,
        })
    }
}
```

---

### NEW-P1-06 — `free_pd_tree()` libère les PT tables mais jamais les frames feuilles
**Gravité :** P1 — Fuite mémoire massive  
**Fichier :** `kernel/src/memory/virtual/address_space/fork_impl.rs`

```rust
// CODE ACTUEL — INCOMPLET
unsafe fn free_pd_tree(pd_phys: PhysAddr) {
    let pd = phys_to_table_ref(pd_phys);
    for l2_idx in 0..512 {
        let entry = pd[l2_idx];
        if entry.is_present() && !entry.is_huge() {
            // ↓ pd[l2_idx].phys_addr() = adresse d'une PT (niveau 1), PAS d'une frame
            // → Libère les PT tables mais JAMAIS les frames de données
            let _ = buddy::free_pages(Frame::containing(entry.phys_addr()), 0);
        }
    }
    let _ = buddy::free_pages(Frame::containing(pd_phys), 0);
}
```

La hiérarchie est : PML4 → PDPT → **PD** → **PT** → **Frame**.
`free_pd_tree(pd_phys)` reçoit une PD, boucle sur ses entries (qui pointent vers des PTs),
et libère ces PTs. Mais les frames feuilles (les vraies pages de données : code, stack, heap)
ne sont jamais atteintes.

À chaque `exit()` d'un processus fils fork(), toutes ses pages de données restent
allouées dans le buddy allocator. Sur un système qui fork/exit fréquemment (démarrage
de services Ring1), la mémoire physique diminue continuellement jusqu'à l'OOM.

**Correction :**

```rust
// Nouvelle fonction pour libérer les frames feuilles (niveau PT)
unsafe fn free_pt_frames(pt_phys: PhysAddr) {
    let pt = phys_to_table_ref(pt_phys);
    for l1_idx in 0..512 {
        let entry = pt[l1_idx];
        if entry.is_present() {
            // Décrémenter refcount. Libérer uniquement si refcount atteint 0
            // (la frame peut être partagée via CoW avec le parent)
            let frame = Frame::containing(entry.phys_addr());
            if crate::memory::physical::frame::ref_count::dec_and_check(frame) == 0 {
                let _ = buddy::free_pages(frame, 0);
            }
        }
    }
    // Libérer la PT table elle-même
    let _ = buddy::free_pages(Frame::containing(pt_phys), 0);
}

unsafe fn free_pd_tree(pd_phys: PhysAddr) {
    let pd = phys_to_table_ref(pd_phys);
    for l2_idx in 0..512 {
        let entry = pd[l2_idx];
        if entry.is_present() {
            if entry.is_huge() {
                // Page 2MB : libérer la frame directement
                let frame = Frame::containing(entry.phys_addr());
                if crate::memory::physical::frame::ref_count::dec_and_check(frame) == 0 {
                    let _ = buddy::free_pages(frame, 9); // order 9 = 2MB
                }
            } else {
                // Descendre au niveau PT pour libérer les frames feuilles
                free_pt_frames(entry.phys_addr()); // ← AJOUT CRITIQUE
            }
        }
    }
    let _ = buddy::free_pages(Frame::containing(pd_phys), 0);
}
```

---

### NEW-P1-07 — `clone_pt()` : pas d'`inc_refcount` sur les frames partagées
**Gravité :** P1  
**Fichier :** `kernel/src/memory/virtual/address_space/fork_impl.rs`

Sans `inc_refcount()` dans `clone_pt()`, le buddy allocator ne sait pas que deux espaces
d'adressage partagent la même frame. Quand le fils fait un CoW break (écriture sur une
page partagée), le handler CoW alloue une nouvelle frame, copie le contenu, et appelle
`dec_refcount()`. Si le refcount initial était 1 (non incrémenté), il passe à 0 → la
frame est libérée alors que le parent la mappe encore → use-after-free.

Cette correction est couplée à NEW-P0-06 : l'`inc_refcount` doit être ajouté
dans `clone_pt()` simultanément au retrait de `FLAG_WRITABLE`. Voir le code de
correction dans NEW-P0-06.

---

### NEW-P1-08 — 4 serveurs Ring1 utilisent `SYS_IPC_SEND = 302` qui est `RECV_NB` côté kernel
**Gravité :** P1  
**Fichiers :**
- `servers/ipc_router/src/main.rs:64–66`
- `servers/vfs_server/src/main.rs:52–54`
- `servers/crypto_server/src/main.rs:60–62`
- `servers/exo_shield/src/main.rs:55–57`

**Code actuel (identique dans les 4 serveurs) :**
```rust
mod syscall {
    pub const SYS_IPC_REGISTER: u64 = 300; // kernel: SYS_EXO_IPC_SEND
    pub const SYS_IPC_RECV:     u64 = 301; // kernel: SYS_EXO_IPC_RECV ✓
    pub const SYS_IPC_SEND:     u64 = 302; // kernel: SYS_EXO_IPC_RECV_NB ← FAUX
}
```

**Table de divergence kernel vs serveurs :**

| Numéro | Kernel `numbers.rs` | 4 serveurs (local) | Résultat appelé |
|--------|---------------------|--------------------|-----------------|
| 300 | `SYS_EXO_IPC_SEND` | `SYS_IPC_REGISTER` | Envoi IPC au lieu d'enregistrement |
| 301 | `SYS_EXO_IPC_RECV` | `SYS_IPC_RECV` | ✓ Correct |
| 302 | `SYS_EXO_IPC_RECV_NB` | `SYS_IPC_SEND` | Réception non-bloquante au lieu d'envoi |
| 304 | `SYS_EXO_IPC_CREATE` | (absent) | Non appelable |

Au démarrage, chaque serveur pense s'enregistrer avec `syscall(300, ...)` mais le kernel
reçoit un `SYS_EXO_IPC_SEND` → le nom du serveur est interprété comme un payload IPC.
Dans la boucle de traitement, chaque forward de message via `syscall(302, ...)` déclenche
un `SYS_EXO_IPC_RECV_NB` → le ring SPSC est vide à cet instant → retourne `EAGAIN` →
le message est silencieusement perdu.

**Correction :** Créer la crate `servers/syscall_abi/src/lib.rs` partagée :

```rust
// servers/syscall_abi/src/lib.rs
#![no_std]

/// Source unique de vérité pour les numéros de syscall ExoOS.
/// DOIT être synchronisée avec kernel/src/syscall/numbers.rs.

// POSIX de base
pub const SYS_READ:    u64 = 0;
pub const SYS_WRITE:   u64 = 1;
pub const SYS_OPEN:    u64 = 2;
pub const SYS_CLOSE:   u64 = 3;
pub const SYS_FORK:    u64 = 57;
pub const SYS_EXECVE:  u64 = 59;
pub const SYS_EXIT:    u64 = 60;

// IPC natif ExoOS (bloc 300+)
pub const SYS_EXO_IPC_SEND:    u64 = 300;
pub const SYS_EXO_IPC_RECV:    u64 = 301;
pub const SYS_EXO_IPC_RECV_NB: u64 = 302;
pub const SYS_EXO_IPC_CALL:    u64 = 303;
pub const SYS_EXO_IPC_CREATE:  u64 = 304; // Enregistrement d'endpoint
pub const SYS_EXO_IPC_DESTROY: u64 = 305;
```

Remplacer les modules `mod syscall { ... }` locaux dans les 4 serveurs par
`use syscall_abi::*;` et corriger les usages :
- `SYS_IPC_REGISTER` → `SYS_EXO_IPC_CREATE` (304)
- `SYS_IPC_SEND`     → `SYS_EXO_IPC_SEND` (300)
- `SYS_IPC_RECV`     → `SYS_EXO_IPC_RECV` (301) ← déjà correct

---

### NEW-P2-06 — Limite effective IPC 64 octets mais validation annonce 65536
**Gravité :** P2  
**Fichier :** `kernel/src/syscall/table.rs:725–730`

```rust
pub fn sys_exo_ipc_send(endpoint: u64, msg_ptr: u64, msg_len: u64, ...) -> i64 {
    let len = msg_len as usize;
    if len > 65536 { return E2BIG; }            // ← jamais déclenché
    if len > IpcFastMsg::zeroed().data.len() {  // data.len() = 64
        return EINVAL;                          // ← déclenché dès len > 64
    }
    // ...
}
```

`IpcFastMsg.data = [u8; 64]`, donc toute tentative d'envoi de plus de 64 octets retourne
`EINVAL` (argument invalide) au lieu de `E2BIG` (message trop grand). La vérification
E2BIG à 65536 est lettre morte. Un serveur Ring1 qui envoie 128 octets en pensant que
la limite documentée est 64KB reçoit `EINVAL` sans diagnostic clair.

**Correction :**
```rust
const IPC_FAST_MAX: usize = core::mem::size_of::<IpcFastMsg>() - 4; // 60B après header

pub fn sys_exo_ipc_send(endpoint: u64, msg_ptr: u64, msg_len: u64, ...) -> i64 {
    let len = msg_len as usize;
    if len > IPC_FAST_MAX {
        // Retourner E2BIG avec un message clair — pas EINVAL
        return E2BIG; // → EMSGSIZE côté POSIX
    }
    // ...
}
```

Mettre à jour la documentation des syscalls pour indiquer explicitement la limite
de 60 octets pour les fast IPC, et la voie à suivre pour les messages plus grands
(SHM ou batch ring).

---

### NEW-P2-07 — `ThreadControlBlock` manque le champ `creation_tsc` (P2-04 incomplet)
**Gravité :** P2  
**Fichier :** `kernel/src/scheduler/core/task.rs` + `kernel/src/security/exoledger.rs`

La correction P2-04 documentée dans FIX-4 requiert `tcb.creation_tsc` pour rendre
les OIDs d'`exoledger.rs` non-ambigus après réutilisation de PID. Ce champ n'existe
pas dans `ThreadControlBlock`. Le TCB a une contrainte de taille stricte à 256 bytes
avec assertion compile-time.

Le champ peut être logé dans `_cold_reserve[24..32]` (offset TCB 168, actuellement zéro)
sans modifier le layout :

```rust
// scheduler/core/task.rs

// Dans ThreadControlBlock, remplacer l'utilisation de _cold_reserve[24..32] :
// Les offsets _cold_reserve[0..24] sont déjà utilisés par ExoShield (shadow_stack_token,
// cet_flags, threat_score_u8, pt_buffer_phys). Les octets [24..32] sont libres.

// Helper pour lire/écrire creation_tsc via _cold_reserve
impl ThreadControlBlock {
    /// TSC de création du thread — stocké dans _cold_reserve[24..32] (offset TCB 168).
    pub fn creation_tsc(&self) -> u64 {
        u64::from_le_bytes(self._cold_reserve[24..32].try_into().unwrap())
    }

    pub fn set_creation_tsc(&mut self, tsc: u64) {
        self._cold_reserve[24..32].copy_from_slice(&tsc.to_le_bytes());
    }
}

// Dans new() ou wherever threads are created:
tcb.set_creation_tsc(crate::arch::x86_64::cpu::tsc::read_tsc());
```

Puis dans `exoledger.rs::current_actor_oid()` :
```rust
oid[16..24].copy_from_slice(&tcb.creation_tsc().to_le_bytes());
```

---

## 4. Plan de correction consolidé

### Phase 1 — Débloque le userspace (P0 absolus)

| Ordre | ID | Fichier principal | Description |
|-------|----|-------------------|-------------|
| 1 | NEW-P0-06 | `fork_impl.rs` — `clone_pt()` | Retirer `FLAG_WRITABLE`, poser `FLAG_COW`, `inc_refcount()` |
| 2 | NEW-P2-08* | `fork_impl.rs` — `clone_pdpt/pd()` | Même correction pour les huge pages |
| 3 | NEW-P1-07 | `fork_impl.rs` | Vérifier que `inc_refcount` est cohérent avec CoW handler |
| 4 | NEW-P0-07 | `elf_loader_impl.rs` | Implémenter chargement ELF réel via ExoFS |
| 5 | NEW-P1-08 | 4 serveurs | Créer `syscall_abi` crate, corriger numéros IPC |

### Phase 2 — Stabilité mémoire (P1)

| Ordre | ID | Fichier principal | Description |
|-------|----|-------------------|-------------|
| 6 | NEW-P1-06 | `fork_impl.rs` — `free_pd_tree()` | Ajouter `free_pt_frames()` pour libérer les feuilles |
| 7 | EXT-02 | `vfs_compat.rs` — `OpenFdTable` | Remplacer `Vec<VfsFd>` par tableau fixe borné |
| 8 | EXT-03 | `runqueue.rs` — `run_queue()` | Remplacer `debug_assert!` par `assert!` + `AtomicBool` |

### Phase 3 — Robustesse et observabilité (P2)

| Ordre | ID | Fichier principal | Description |
|-------|----|-------------------|-------------|
| 9 | EXT-01 | 13 fichiers ExoFS | Réactiver les 16 `SIZE_ASSERT_DISABLED` |
| 10 | NEW-P2-07 | `task.rs` + `exoledger.rs` | Ajouter `creation_tsc` dans `_cold_reserve[24..32]` |
| 11 | NEW-P2-06 | `table.rs` — `sys_exo_ipc_send()` | Retourner `E2BIG` au lieu de `EINVAL`, documenter limite 60B |
| 12 | EXT-04 | `fs/exofs/mod.rs` | Supprimer `static mut EXOFS_SUPERBLOCK` (dead code) |

---

## Annexe — Récapitulatif de vérité sur les rapports externes D1–D5

| Allégation | Source | Vrai dans le code ? | Raison |
|------------|--------|---------------------|--------|
| SSR_MAX_CORES_LAYOUT divergence | D1/D2/D3 | ❌ Faux | Unifié à 256 dans la crate partagée |
| security::init() non câblé | D1/D2/D3 | ❌ Faux | Câblé en lib.rs:230 |
| init_syscall() BSP seulement | D1/D2/D3 | ❌ Faux | Câblé pour APs en smp/init.rs:127 |
| gs:[0x20] non écrit | D1/D2/D3 | ❌ Faux | Mis à jour en switch.rs:272 |
| check_sys_admin = true dur | D1/D2/D3 | ❌ Faux | Vérifie pcb.is_root() |
| md_mmio_whitelist = true dur | D1/D2/D3 | ❌ Faux | Parcourt MEMORY_MAP Reserved |
| static mut IDT race SMP | D5 | ❌ Faux | Write-once BSP, read-only après init |
| MAX_CPUS = 3 valeurs | D5 | ❌ Faux | Toutes les def MAX_CPUS = 256 |
| TSS.RSP0 non mis à jour | D5 | ❌ Faux | update_rsp0() en switch.rs:301 |
| Lazy FPU non géré | D5 | ❌ Faux | XSAVE + CR0.TS présents en switch.rs |
| TOCTOU sys_pci_claim | D1/D5 | ❌ Faux | Lock avant vérifications (CORR-32) |
| verify_cap_token non CT | D1/D5 | ❌ Faux | subtle::ConstantTimeEq + Denied |
| Stack canaries absentes | D5 | ❌ Faux | memory/integrity/canary.rs présent |
| static mut EXOFS_SUPERBLOCK race | D5 | ❌ Faux (dead code) | Jamais lue (#allow(dead_code)) |
| **16 SIZE_ASSERT_DISABLED** | **D3/D5** | **✅ Vrai** | **Commentées dans 13 fichiers ExoFS** |
| **OpenFdTable Vec non borné** | **D3/D5** | **✅ Vrai** | **`Vec<VfsFd>` sans limite ni borne** |
| **run_queue debug_assert release** | **D5** | **✅ Vrai** | **debug_assert = no-op en --release** |
| **EXOFS_SUPERBLOCK dead code** | **D5** | **✅ Partiel** | **Dead code, pas de race — mais à nettoyer** |

---

*Rapport d'audit croisé ExoOS — Commit `74c3659e` — 20 avril 2026*  
*Méthodologie : lecture directe code source + vérification grep + cross-référence docs/recast*
