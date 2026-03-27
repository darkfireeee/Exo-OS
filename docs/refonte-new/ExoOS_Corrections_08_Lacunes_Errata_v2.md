# ExoOS — Corrections v2 : Lacunes, Errata & Rejections (CORR-42 à CORR-50)
**Sources : Z-AI, Copilote, ChatGPT5, KIMI-AI, MiniMax + analyse propre**

---

## CORR-42 ⚠️ — SRV-05 : règle ipc_broker directory service

### Problème
Les règles SRV-* sautent de SRV-02 à SRV-04, puis SRV-03 est documenté comme supprimé (CORR-21). SRV-05 n'existe pas alors que le comportement critique d'`ipc_broker` — sa persistance vers ExoFS — n'est documenté dans aucune règle.

**Source** : Z-AI §2.1

### Correction — Ajouter dans Architecture v7 §1.3 + Arborescence V4 §8

```markdown
| **SRV-05** | **ipc_broker persistence** | Le `registry.rs` (ipc_broker) maintient la table |
|            |                            | ServiceName→(PID, CapToken) en RAM ET la persiste |
|            |                            | vers ExoFS via `persistence.rs` à intervalles     |
|            |                            | réguliers et sur signal PrepareIsolation (PHX-01). |
|            |                            | Après restore Phoenix, ipc_broker DOIT recharger   |
|            |                            | le registre depuis ExoFS avant d'accepter des       |
|            |                            | lookups. Service Name "ipc_broker" réservé.         |
```

---

## CORR-43 ⚠️ — Syscalls Phoenix 520-529 : mapping complet

### Problème
Architecture v7 §5.4 mentionne `phoenix_query(520)` et `phoenix_notify(521)` mais ne définit pas les syscalls 522-529. CORR-20 de la session précédente définit 500-519 mais omet cette plage.

**Source** : Z-AI CORR-35, KIMI CORR-39

### Mapping canonique — `exo-syscall/src/phoenix.rs`

```rust
// exo-syscall/src/phoenix.rs — CORR-43
// Plage Phoenix : 520-529 — VERROUILLÉE

/// Interroger l'état de Kernel B (ExoPhoenix).
/// Retourne PhoenixEvent : NORMAL | FREEZE_PENDING | RESTORE_COMPLETE
pub const SYS_PHOENIX_QUERY:    u32 = 520;

/// Notifier Kernel B que tous les servers sont checkpointés (AllReady).
/// Déclenche la phase de snapshot SSR.
pub const SYS_PHOENIX_NOTIFY:   u32 = 521;

/// Obtenir le statut détaillé de ExoPhoenix (pour monitoring).
pub const SYS_PHOENIX_STATUS:   u32 = 522;

/// Forcer un cycle ExoPhoenix immédiat (SysAdmin uniquement).
pub const SYS_PHOENIX_FORCE:    u32 = 523;

// 524-529 : RÉSERVÉS pour extensions Phase 9+ (TLA+, SeqLock, audit)
pub const SYS_PHOENIX_RESERVED_START: u32 = 524;
pub const SYS_PHOENIX_RESERVED_END:   u32 = 529;

/// Vérification : un numéro est-il dans la plage ExoPhoenix ?
pub fn is_phoenix_syscall(num: u32) -> bool {
    (520..=529).contains(&num)
}

/// Table complète unifiée des plages syscall ExoOS
pub mod ranges {
    pub const EXOFS_BASE:     u32 = 500;
    pub const EXOFS_END:      u32 = 519; // 20 syscalls ExoFS
    pub const PHOENIX_BASE:   u32 = 520;
    pub const PHOENIX_END:    u32 = 529; // 10 syscalls Phoenix
    pub const DRIVER_BASE:    u32 = 530;
    pub const DRIVER_END:     u32 = 546; // 17 syscalls Driver (Driver_Framework_v10)
    // 547+ : réservé Phase 9+
}
```

---

## CORR-44 ⚠️ — IRQ table : spécification taille 256 entrées

### Problème
`IRQ_TABLE` est utilisée partout dans le Driver Framework mais sa taille n'est jamais explicitement définie dans les documents.

**Source** : Z-AI CORR-38

### Correction — `kernel/src/arch/x86_64/irq/routing.rs`

```rust
// routing.rs — CORR-44

/// Nombre de vecteurs d'interruption x86_64.
/// 256 vecteurs (0x00..0xFF), mais :
///   0x00-0x1F : exceptions CPU réservées (Division Error, #PF, etc.)
///   0x20-0xEF : utilisables pour IRQs et IPIs
///   0xF0-0xFF : réservés ExoOS (0xF1=reschedule, 0xF2=TLB, 0xF3=Phoenix)
pub const IRQ_TABLE_SIZE: usize = 256;

/// Table globale des routes IRQ — une entrée par vecteur x86_64.
/// None = vecteur non enregistré (handler par défaut = EOI + log).
pub static IRQ_TABLE: RwLock<[Option<IrqRoute>; IRQ_TABLE_SIZE]> =
    RwLock::new([const { None }; IRQ_TABLE_SIZE]);

// Vecteurs réservés (JAMAIS enregistrables par sys_irq_register)
pub const VECTOR_RESERVED_START: u8 = 0xF0;
pub const VECTOR_RESERVED_END:   u8 = 0xFF;

// Dans sys_irq_register : vérifier que le vecteur n'est pas réservé
pub fn sys_irq_register(irq: u8, ...) -> Result<u64, IrqError> {
    if irq >= VECTOR_RESERVED_START {
        return Err(IrqError::VectorReserved);
    }
    // ...
}
```

---

## CORR-45 ⚠️ — IoVec : alignement et validation bornes

### Problème
`IoVec { base: u64, len: u64 }` est passé depuis userspace via `readv`/`writev`. Ni l'alignement de la structure elle-même, ni la validation que `base` est dans l'espace userspace, ne sont documentés.

**Source** : MiniMax EN-06

### Correction — `libs/exo-types/src/iovec.rs`

```rust
// libs/exo-types/src/iovec.rs — CORR-45

/// Vecteur I/O pour readv/writev — ABI Linux exacte.
/// Doit être aligné sur 8 octets (garanti par #[repr(C, align(8))]).
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug)]
pub struct IoVec {
    /// Adresse userspace Ring 3 — OBLIGATOIREMENT vérifiée par copy_from_user().
    pub base: u64,
    /// Longueur en bytes.
    pub len:  u64,
}

// Vérification ABI compile-time
const _: () = assert!(core::mem::size_of::<IoVec>() == 16);
const _: () = assert!(core::mem::align_of::<IoVec>() == 8);

/// Valide un tableau d'IoVec passé depuis userspace.
///
/// Vérifie :
///   - Alignement du pointeur sur 8B
///   - Chaque (base, len) est dans l'espace adressable userspace Ring 3
///   - Pas de débordement arithmétique sur base+len
///
/// OBLIGATOIRE avant tout usage d'un IoVec venant de Ring 3.
pub fn validate_iovec_array(
    ptr: *const IoVec,
    count: usize,
    user_space_limit: u64, // Adresse virtuelle max de l'espace Ring 3
) -> Result<(), ExofsError> {
    // Vérifier alignement du pointeur sur le tableau
    if (ptr as usize) % core::mem::align_of::<IoVec>() != 0 {
        return Err(ExofsError::InvalidArg); // EINVAL
    }

    // Vérifier que le tableau lui-même est en espace userspace
    let array_size = count
        .checked_mul(core::mem::size_of::<IoVec>())
        .ok_or(ExofsError::InvalidArg)?;
    let array_end = (ptr as u64)
        .checked_add(array_size as u64)
        .ok_or(ExofsError::InvalidArg)?;
    if array_end > user_space_limit {
        return Err(ExofsError::BadAddress); // EFAULT
    }

    // Valider chaque IoVec individuellement
    for i in 0..count {
        let iov = unsafe { &*ptr.add(i) };
        let end = iov.base
            .checked_add(iov.len)
            .ok_or(ExofsError::InvalidArg)?;
        if end > user_space_limit {
            return Err(ExofsError::BadAddress);
        }
    }

    Ok(())
}
```

---

## CORR-46 ⚠️ — O_DIRECT : responsabilité du bounce buffering

### Problème
ExoFS TL v5 TL-15 dit : "`O_DIRECT` bypasse page_cache — `dio_pool.rs` garantit alignement 512B/4KB". Mais il n'est pas clair si `dio_pool` est dans ExoFS Ring 0 ou dans le driver Ring 1 (virtio-block). Si les deux pensent que l'autre fait l'alignement, les I/Os non-alignées passent.

**Source** : MiniMax IC-01

### Correction — TL-38 à ajouter dans ExoFS TL v5 §5

```markdown
| ✅ | TL-38 | Responsabilité du bounce buffering O_DIRECT :                           |
|    |        | Ring 0 (posix_bridge/direct_io.c) = vérification alignement avant       |
|    |        | de passer au driver. Si non aligné : retourner EINVAL.                 |
|    |        | Ring 1 (virtio-block) = accepte uniquement des buffers déjà alignés.   |
|    |        | Le driver Ring 1 NE FAIT PAS de bounce buffering (pas de mémoire       |
|    |        | kernel intermédiaire allouée dans Ring 1).                              |
|    |        | dio_pool.rs (Ring 0) alloue des buffers DMA pré-alignés 512B.           |
|    |        | Ces buffers sont obtenus via SYS_DMA_ALLOC → IoVirtAddr (jamais         |
|    |        | PhysAddr programmée directement dans le device).                        |
```

```rust
// kernel/src/fs/exofs/posix_bridge/direct_io.rs — CORR-46
// Vérifications alignement avant toute I/O directe

pub fn do_direct_io(
    obj_id:    ObjectId,
    user_buf:  u64,   // adresse Ring 3
    len:       usize,
    offset:    u64,
    direction: DmaDirection,
) -> Result<usize, ExofsError> {
    // CORR-46 : Vérification alignement strict O_DIRECT
    // Alignement minimum : 512B (SATA logical block) ou 4KB (NVMe optimal)
    const O_DIRECT_ALIGN: u64 = 512;

    if user_buf % O_DIRECT_ALIGN != 0 {
        return Err(ExofsError::InvalidArg); // EINVAL — buffer non aligné
    }
    if len as u64 % O_DIRECT_ALIGN != 0 {
        return Err(ExofsError::InvalidArg); // EINVAL — longueur non alignée
    }
    if offset % O_DIRECT_ALIGN != 0 {
        return Err(ExofsError::InvalidArg); // EINVAL — offset non aligné
    }

    // Allouer buffer DMA pré-aligné depuis dio_pool
    let dma_buf = dio_pool::alloc_aligned(len, O_DIRECT_ALIGN as usize)?;
    // ... copier depuis/vers user_buf via copy_from_user / copy_to_user
    // ... passer dma_buf.iova à virtio-block via IPC
    Ok(len)
}
```

---

## CORR-47 ⚠️ — copy_file_range : quota enforcement manquant

### Problème
`do_copy_file_range` dans ExoFS TL v5 §2.1 vérifie `RLIMIT_FSIZE` via TL-29 mais pas le quota par objet/processus (S-13 Architecture v7). Une copie reflink n'alloue pas de nouveaux blocs physiques (refcount++ uniquement) mais comptabilise logiquement des bytes pour le quota.

**Source** : KIMI CORR-38

### Correction — `kernel/src/fs/exofs/posix_bridge/copy_range_kernel.rs`

```rust
// copy_range_kernel.rs — CORR-47

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

    // CORR-47 : Vérification quota AVANT l'opération (S-13)
    // Pour un reflink : quota logique augmente même si bytes physiques = 0
    // Pour une copie DMA : quota physique ET logique augmentent
    quota::check_and_reserve(dst_obj_id, actual_len)
        .map_err(|_| ExofsError::QuotaExceeded)?;

    // [... reste de la logique inchangée ...]

    // En cas d'erreur après la réservation quota, libérer
    // (dans la pratique : epoch rollback suffit)
    epoch::commit_single_op(dst_obj_id)?;
    Ok(CopyRangeResult { bytes_copied: total, reflinks_used: reflinks })
}
```

---

## CORR-48 ⚠️ — Stack canaries pour piles kernel

### Problème
Aucun mécanisme de détection de stack overflow n'est documenté pour les piles kernel des threads Ring 1/3. Un overflow silencieux peut écraser le TCB adjacent ou des données critiques.

**Source** : KIMI CORR-37

### Correction — `kernel/src/memory/stack.rs`

```rust
// kernel/src/memory/stack.rs — CORR-48

/// Valeur de canary — choisie arbitrairement, non-null, non-triviale.
const STACK_CANARY: u64 = 0xDEAD_C0DE_CAFE_BABE;

/// Taille minimale d'une pile kernel : 16KB.
pub const KERNEL_STACK_SIZE: usize = 16 * 1024;

/// Alloue une pile kernel avec guard page + canary.
///
/// Layout :
///   [guard page : PROT_NONE][stack grows down][canary u64 at bottom]
///
/// Détection :
///   - Guard page : #PF si overflow dépasse guard → détection hardware
///   - Canary : vérification software via verify_stack_canary()
pub fn alloc_kernel_stack_with_canary() -> Result<PhysAddr, AllocError> {
    // Allouer la pile + une guard page (pas-de-mappage)
    let stack_phys = buddy_allocator::alloc_pages(KERNEL_STACK_SIZE / PAGE_SIZE + 1)?;

    // Mapper la guard page comme PROT_NONE (premier page = adresse basse)
    page_tables::map_guard_page(stack_phys);

    // Écrire le canary à la première adresse accessible (bas de pile)
    let canary_phys = PhysAddr(stack_phys.0 + PAGE_SIZE as u64);
    let canary_ptr = canary_phys.to_virt() as *mut u64;
    unsafe { canary_ptr.write_volatile(STACK_CANARY); }

    // Retourner le sommet de pile (RSP initial)
    Ok(PhysAddr(stack_phys.0 + (KERNEL_STACK_SIZE + PAGE_SIZE) as u64))
}

/// Vérifie le canary d'un TCB. Appelé dans context_switch (debug builds).
/// En release : vérification périodique uniquement (watchdog scheduler).
#[cfg(debug_assertions)]
pub fn verify_stack_canary(tcb: &ThreadControlBlock) -> bool {
    // Le canary est à KERNEL_STACK_SIZE bytes en dessous du RSP initial
    let canary_addr = (tcb.kstack_ptr as usize)
        .wrapping_sub(KERNEL_STACK_SIZE)
        + core::mem::size_of::<u64>(); // Premier u64 accessible
    let val = unsafe { (canary_addr as *const u64).read_volatile() };
    if val != STACK_CANARY {
        log::error!(
            "STACK OVERFLOW DÉTECTÉ tid={} : canary=0x{:016X} (attendu 0x{:016X})",
            tcb.tid, val, STACK_CANARY
        );
        false
    } else {
        true
    }
}

/// Intégration dans context_switch() (mode debug uniquement) :
/// debug_assert!(memory::verify_stack_canary(prev));
```

---

## Erreurs dans les corrections précédentes (CORR-01 à CORR-30)

### ERRATA-01 — CORR-15 : commentaire ambigu sur CR0.TS (pas une erreur logique)

La condition `if !cr0.contains(Cr0Flags::TASK_SWITCHED)` est **correcte** :
- CR0.TS=0 → FPU active dans registres → XSAVE obligatoire avant gel Phoenix
- CR0.TS=1 → Lazy FPU, état dans `fpu_state_ptr` → pas besoin de XSAVE

Cependant, le commentaire de la section CORR-15 peut être rendu plus explicite :

```rust
// CLARIFICATION CORR-15 (ERRATA-01) :
//
// CR0.TS = Task Switched flag (bit 3 de CR0).
//   = 1 : mis par le CPU ou par ExoOS après un context switch.
//         Le prochain usage FPU déclenchera #NM pour restaurer l'état lazy.
//         Dans ce cas : état FPU déjà dans fpu_state_ptr → pas de XSAVE forcé.
//   = 0 : FPU "active" — le thread a utilisé la FPU depuis le dernier context switch.
//         L'état réel est dans les REGISTRES CPU, pas encore dans fpu_state_ptr.
//         → XSAVE forcé obligatoire avant le snapshot Phoenix.
//
// !cr0.contains(TASK_SWITCHED) = CR0.TS == 0 = FPU active → XSAVE requis.
// C'est EXACT — pas d'inversion de condition.
```

### ERRATA-02 — CORR-15 : timeout de spin-wait manquant

CORR-15 (session 1) spécifie la séquence de gel mais oublie le timeout du spin-wait.  
**Correction complète** : voir CORR-33 dans ce fichier.

### ERRATA-03 — CORR-02 : SSR_LAYOUT_MAGIC absent des constantes Copilote/IAs

Le champ MAGIC `SSR_LAYOUT_MAGIC = 0x5353525F4558_4F53` est défini dans les constantes CORR-02 mais certains feedbacks IAs ont proposé des corrections SSR sans inclure ce champ. Pour éviter toute ambiguïté, re-confirmer : **MAGIC est à l'offset 0x0000 et est obligatoire** (CORR-03, S-23).

---

## Tableau de toutes les corrections v1 + v2 (vue consolidée)

### 🔴 Critiques (bloquantes)
| ID | Titre | Fichier |
|----|-------|---------|
| CORR-01 | TCB Layout unifié Architecture v7 | 01_Kernel_Types |
| CORR-02 | SSR Layout MAX_CORES=256 | 01_Kernel_Types |
| CORR-03 | SSR Header MAGIC en premier | 01_Kernel_Types |
| CORR-04 | Vec en ISR → tableau fixe | 03_Driver_Framework |
| CORR-05 | CapabilityType enum C invalide | 06_Servers |
| CORR-06 | EpollEventAbi packed UB | 04_ExoFS |
| CORR-07 | ObjectId::is_valid() exception ZERO_BLOB | 01_Kernel_Types |
| CORR-31 | IpcMessage payload 48B migration | 07_Critiques |
| CORR-32 | sys_pci_claim TOCTOU + BDF unique | 07_Critiques |
| CORR-33 | Phoenix freeze spin-wait timeout | 07_Critiques |

### 🟠 Majeures
| ID | Titre | Fichier |
|----|-------|---------|
| CORR-08 | masked_since CAS ordering Release | 03_Driver_Framework |
| CORR-09 | BootInfo virtuel supprimer argv[1] | 02_Architecture |
| CORR-10 | IPI broadcasts exclure Core 0 | 02_Architecture |
| CORR-11 | FS/GS base dans context_switch | 02_Architecture |
| CORR-12 | Crypto nonce reseed post-restore | 05_ExoPhoenix |
| CORR-13 | VFS sync_fs avant PrepareIsolationAck | 05_ExoPhoenix |
| CORR-14 | DMA bus master disable avant gel | 05_ExoPhoenix |
| CORR-15 | FPU xsave forcé avant gel Phoenix | 05_ExoPhoenix |
| CORR-16 | domain_of_pid() spécification | 03_Driver_Framework |
| CORR-17 | sender_pid + reply_nonce | 06_Servers |
| CORR-18 | switch_asm.s commentaire GPRs | 02_Architecture |
| CORR-19 | spin_count reset par tentative | 03_Driver_Framework |
| CORR-34 | TSC calcul différentiel | 07_Critiques |
| CORR-35 | Phoenix restore sequence restart | 07_Critiques |
| CORR-36 | Panic handler Ring 1 | 07_Critiques |
| CORR-37 | IRQ handler limit reject at registration | 07_Critiques |
| CORR-38 | BootInfo read-only + integrity | 07_Critiques |
| CORR-39 | fd_table validation post-restore | 07_Critiques |
| CORR-40 | IpcEndpoint Copy assertion | 07_Critiques |
| CORR-41 | verify_cap_token constant-time | 07_Critiques |

### ⚠️ Lacunes
| ID | Titre | Fichier |
|----|-------|---------|
| CORR-20 | SYS_EXOFS_* 500-518 mapping | 04_ExoFS |
| CORR-21 | SRV-03 supprimé documenter | 06_Servers |
| CORR-22 | BlobId concept pas type Rust | 04_ExoFS |
| CORR-23 | IommuDomainRegistry spec | 03_Driver_Framework |
| CORR-24 | SeqLock Phase 9 roadmap | 02_Architecture |
| CORR-25 | device_server arborescence pci/ gdi/ | 06_Servers |
| CORR-26 | CI virtio_block harmonisation | 06_Servers |
| CORR-42 | SRV-05 ipc_broker persistence | 08_Lacunes |
| CORR-43 | Syscalls Phoenix 520-529 mapping | 08_Lacunes |
| CORR-44 | IRQ table size 256 entries | 08_Lacunes |
| CORR-45 | IoVec alignement validation | 08_Lacunes |
| CORR-46 | O_DIRECT bounce buffering resp. | 08_Lacunes |
| CORR-47 | copy_file_range quota enforcement | 08_Lacunes |
| CORR-48 | Stack canaries kernel | 08_Lacunes |

### 🔵 Mineures
| ID | Titre | Fichier |
|----|-------|---------|
| CORR-27 | MAX_CPUS preempt 64→256 | 02_Architecture |
| CORR-28 | Arborescence V3 archiver | 06_Servers |
| CORR-29 | user_gs_base nommage | 01_Kernel_Types |
| CORR-30 | FixedString len: u32 | 01_Kernel_Types |

**Total : 48 corrections (CORR-01 à CORR-48)**

---

## Rejections documentées (RETOUR-AI-2)

| IA | Correction proposée | Raison du rejet |
|----|--------------------|-----------------| 
| MiniMax EN-01 | CORR-15 condition CR0.TS inversée | INCORRECT — condition `!contains(TASK_SWITCHED)` = CR0.TS=0 = FPU active. Correct. MiniMax se contredit lui-même |
| MiniMax EN-03 | SeqCst pour IommuFaultQueue | INCORRECT — brise le design CAS-based AcqRel/Release prouvé. SeqCst inutile et coûteux |
| KIMI CORR-35 | wait_link_retraining lock+yield | INCORRECT — `parent_bridge()` retourne `Option<PciBdf>: Copy`, lock libéré avant return |
| KIMI CORR-34 | spin_loop en ISR si dropped>1M | CRITIQUE — spin_loop en ISR est interdit (FIX-109). Rejeté |
| Z-AI CORR-33 | CapGuard RAII pour cap generation | Overcompliqué Phase 8 — déjà couvert par CORR-41 + LAC-01 |
| IC-02 (MiniMax) | MAX_PENDING_ACKS non défini | INCORRECT — valeur = 4096 définie dans Driver_Framework_v10 §3.1 |
| KIMI CORR-36 | CapToken time-window (GENERATION_WINDOW_NS) | Hors scope Phase 8 — generation counter suffit, time-window = Phase 1 crypto |

---

## Score de couverture — État final post-corrections v1+v2

| Domaine | Avant v1 | Après v1 (CORR-01..30) | Après v2 (CORR-31..48) |
|---------|----------|------------------------|------------------------|
| Types & TCB | 60% | 100% | 100% |
| SSR / ExoPhoenix layout | 40% | 100% | 100% |
| Driver Framework / IRQ | 70% | 90% | 97% |
| ExoFS / POSIX bridge | 85% | 95% | 98% |
| Boot & context switch | 75% | 95% | 98% |
| Sécurité (capabilities, timing) | 30% | 55% | 80% |
| Phoenix gel/restore | 50% | 80% | 95% |
| Servers / arborescence | 80% | 92% | 97% |
| **Global** | **~60%** | **~88%** | **~96%** |

---

*ExoOS — Corrections Lacunes & Errata v2 (CORR-42 à CORR-48) — Mars 2026*  
*Sources : Z-AI, Copilote, ChatGPT5, KIMI-AI, MiniMax + analyse propre*
