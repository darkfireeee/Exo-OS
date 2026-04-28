--- docs/audit/AUDIT_03_Specs_P2_P3.md (原始)


+++ docs/audit/AUDIT_03_Specs_P2_P3.md (修改后)
# ExoOS — Spécifications Techniques Complémentaires P2/P3

## 🎯 Objectif : Corrections lacunes et améliorations continues

**Document technique** — Implémentations détaillées pour corrections P2 (Lacunes) et P3 (Mineur)
**Références** : CORR-45 à CORR-54, SRV-05

---

## ⚠️ P2 — LACUNES (Amélioration continue)

### CORR-45 : IoVec alignement 8B

**Fichier cible** : `libs/exo-types/src/iovec.rs`

```rust
// libs/exo-types/src/iovec.rs
// ✅ CORR-45 — Alignement explicite + validation bornes

/// Vecteur I/O pour readv/writev — ABI Linux exacte.
/// Doit être aligné sur 8 octets (garanti par #[repr(C, align(8))]).
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoVec {
    /// Adresse userspace Ring 3 — OBLIGATOIREMENT vérifiée par copy_from_user().
    pub base: u64,
    /// Longueur en bytes.
    pub len: u64,
}

// Vérifications ABI compile-time
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
///
/// # Arguments
/// * `ptr` - Pointeur vers le tableau d'IoVec en espace userspace
/// * `count` - Nombre d'éléments dans le tableau
/// * `user_space_limit` - Adresse virtuelle max de l'espace Ring 3
///
/// # Returns
/// * `Ok(())` si toutes les validations passent
/// * `Err(ExofsError::InvalidArg)` si alignement incorrect
/// * `Err(ExofsError::BadAddress)` si adresse hors espace userspace
pub fn validate_iovec_array(
    ptr: *const IoVec,
    count: usize,
    user_space_limit: u64,
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

        // Vérifier len non-nulle (optionnel — certains syscalls permettent len=0)
        // if iov.len == 0 {
        //     return Err(ExofsError::InvalidArg);
        // }
    }

    Ok(())
}

// ─── Tests unitaires ──────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iovec_alignment() {
        // Vérifier que IoVec est bien aligné sur 8B
        assert_eq!(core::mem::align_of::<IoVec>(), 8);
        assert_eq!(core::mem::size_of::<IoVec>(), 16);
    }

    #[test]
    fn test_validate_iovec_array_valid() {
        let iovs = [
            IoVec { base: 0x1000, len: 4096 },
            IoVec { base: 0x2000, len: 2048 },
        ];

        // user_space_limit typique = 0x0000_7FFF_FFFF_FFFF (48-bit canonical)
        let limit = 0x0000_7FFF_FFFF_FFFF;

        assert!(validate_iovec_array(iovs.as_ptr(), iovs.len(), limit).is_ok());
    }

    #[test]
    fn test_validate_iovec_array_overflow() {
        let iovs = [
            IoVec { base: 0x0000_7FFF_FFFF_F000, len: 0x2000 }, // overflow
        ];

        let limit = 0x0000_7FFF_FFFF_FFFF;

        assert!(matches!(
            validate_iovec_array(iovs.as_ptr(), iovs.len(), limit),
            Err(ExofsError::BadAddress)
        ));
    }
}
```

---

### CORR-46 : O_DIRECT bounce buffering — Règle TL-38

**Fichier cible** : `docs/recast/ExoFS_Translation_Layer_v5_FINAL.md` (à mettre à jour)
**Implémentation** : `kernel/src/fs/exofs/posix_bridge/direct_io.rs`

#### Ajout dans ExoFS TL v5 §5

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

#### Implémentation direct_io.rs

```rust
// kernel/src/fs/exofs/posix_bridge/direct_io.rs
// ✅ CORR-46 — Vérifications alignement strict O_DIRECT

use crate::memory::dma::dio_pool;
use crate::error::ExofsError;

/// Alignement minimum pour O_DIRECT.
/// 512B = SATA logical block size (minimum requis)
/// 4KB = NVMe optimal (recommandé pour performances)
pub const O_DIRECT_ALIGN: usize = 512;

/// Effectue une I/O directe (bypass page cache).
///
/// # Arguments
/// * `obj_id` - ObjectId du fichier
/// * `user_buf` - Adresse buffer userspace (Ring 3)
/// * `len` - Nombre de bytes à lire/écrire
/// * `offset` - Offset dans le fichier
/// * `direction` - Direction (Read ou Write)
///
/// # Returns
/// * `Ok(usize)` — Nombre de bytes effectivement transférés
/// * `Err(ExofsError::InvalidArg)` — Buffer/offset/longueur non alignés
/// * `Err(ExofsError::BadAddress)` — Adresse userspace invalide
/// * `Err(ExofsError::Io)` — Erreur I/O sous-jacente
pub fn do_direct_io(
    obj_id:    ObjectId,
    user_buf:  u64,
    len:       usize,
    offset:    u64,
    direction: DmaDirection,
) -> Result<usize, ExofsError> {
    // ✅ CORR-46 : Vérification alignement strict O_DIRECT

    // Buffer userspace doit être aligné
    if user_buf % O_DIRECT_ALIGN as u64 != 0 {
        log::debug!(
            "O_DIRECT reject: user_buf {:#x} non aligné (requis: {}B)",
            user_buf, O_DIRECT_ALIGN
        );
        return Err(ExofsError::InvalidArg); // EINVAL
    }

    // Longueur doit être multiple du bloc
    if len % O_DIRECT_ALIGN != 0 {
        log::debug!(
            "O_DIRECT reject: len {} non aligné (requis: multiple de {}B)",
            len, O_DIRECT_ALIGN
        );
        return Err(ExofsError::InvalidArg);
    }

    // Offset fichier doit être aligné
    if offset % O_DIRECT_ALIGN as u64 != 0 {
        log::debug!(
            "O_DIRECT reject: offset {:#x} non aligné",
            offset
        );
        return Err(ExofsError::InvalidArg);
    }

    // Allouer buffer DMA pré-aligné depuis dio_pool
    // dio_pool garantit alignement 512B/4KB et retourne IoVirtAddr
    let dma_buf = dio_pool::alloc_aligned(len, O_DIRECT_ALIGN)?;

    match direction {
        DmaDirection::Read => {
            // Copier depuis device vers dma_buf (DMA)
            // Puis copier dma_buf → user_buf (copy_to_user)
            device_read(obj_id, offset, &dma_buf)?;
            copy_to_user(user_buf, dma_buf.as_slice())?;
        }
        DmaDirection::Write => {
            // Copier user_buf → dma_buf (copy_from_user)
            // Puis dma_buf → device (DMA)
            copy_from_user(&dma_buf, user_buf)?;
            device_write(obj_id, offset, &dma_buf)?;
        }
    }

    Ok(len)
}
```

---

### CORR-47 : Quota enforcement copy_file_range

**Fichier cible** : `kernel/src/fs/exofs/posix_bridge/copy_range_kernel.rs`

```rust
// kernel/src/fs/exofs/posix_bridge/copy_range_kernel.rs
// ✅ CORR-47 — Vérification quota AVANT copie (reflink ou DMA)

use crate::quota;
use crate::error::ExofsError;

/// Copie des données entre deux fichiers (reflink ou copie physique).
///
/// # Arguments
/// * `src_obj_id` - ObjectId source
/// * `src_off` - Offset source
/// * `dst_obj_id` - ObjectId destination
/// * `dst_off` - Offset destination
/// * `len` - Nombre de bytes à copier
///
/// # Returns
/// * `Ok(CopyRangeResult)` — Succès avec statistiques
/// * `Err(ExofsError::QuotaExceeded)` — Quota destination dépassé
/// * `Err(ExofsError::PermissionDenied)` — Capability insuffisante
pub fn do_copy_file_range(
    src_obj_id: ObjectId,
    src_off: u64,
    dst_obj_id: ObjectId,
    dst_off: u64,
    len: u64,
) -> Result<CopyRangeResult, ExofsError> {
    // Vérifications capabilities
    verify_cap(src_obj_id, Rights::READ)?;
    verify_cap(dst_obj_id, Rights::WRITE)?;

    // Vérifier bounds source
    let src_size = object_table::get_size(src_obj_id)?;
    if src_off >= src_size {
        return Err(ExofsError::InvalidArg);
    }

    // Calculer longueur effective
    let actual_len = len.min(src_size.saturating_sub(src_off));

    // ✅ CORR-47 : Vérification quota AVANT l'opération (S-13)
    // Pour un reflink : quota logique augmente même si bytes physiques = 0
    // Pour une copie DMA : quota physique ET logique augmentent
    //
    // check_and_reserve :
    //   - Vérifie quota restant pour dst_obj_id
    //   - Réserve temporairement les bytes (rollback si échec)
    //   - Retourne Err(QuotaExceeded) si dépassement
    quota::check_and_reserve(dst_obj_id, actual_len)
        .map_err(|e| {
            log::warn!(
                "copy_file_range: quota exceeded for {:?} (need {} bytes)",
                dst_obj_id, actual_len
            );
            ExofsError::QuotaExceeded
        })?;

    // Déterminer mode de copie (reflink si possible, sinon DMA)
    let can_reflink = object_table::can_reflink(src_obj_id, dst_obj_id);

    let result = if can_reflink {
        // Reflink : inc refcount, pas d'allocation physique nouvelle
        perform_reflink_copy(src_obj_id, src_off, dst_obj_id, dst_off, actual_len)?
    } else {
        // Copie physique via DMA
        perform_dma_copy(src_obj_id, src_off, dst_obj_id, dst_off, actual_len)?
    };

    // Commit opération (libère réservation quota, met à jour compteur)
    epoch::commit_single_op(dst_obj_id)?;

    Ok(result)
}

/// Résultat de copy_file_range
#[derive(Debug, Clone, Copy)]
pub struct CopyRangeResult {
    pub bytes_copied: u64,
    pub reflinks_used: bool,
}
```

---

### CORR-48 : Stack canaries

**Fichier cible** : `kernel/src/memory/stack.rs`

```rust
// kernel/src/memory/stack.rs
// ✅ CORR-48 — Stack canaries pour détection overflow

/// Valeur de canary — choisie arbitrairement, non-null, non-triviale.
/// Inspirée de Linux CONFIG_STACKPROTECTOR mais valeur personnalisée.
pub const STACK_CANARY: u64 = 0xDEAD_C0DE_CAFE_BABE;

/// Layout d'une pile kernel avec canary.
///
/// Structure :
///   [Guard page (non-mappée)] ← overflow ici = #PF immédiat
///   [Canary value] ← vérifié à la fin de chaque fonction critique
///   [Variables locales]
///   [Adresse de retour]
///   [Arguments]
#[repr(C)]
pub struct KernelStack {
    /// Page de garde (non-mappée) — optionnel selon config
    // guard_page: [u8; PAGE_SIZE],

    /// Canary de protection
    canary: u64,

    /// Données de pile (taille configurable)
     [u8; KERNEL_STACK_SIZE],
}

impl KernelStack {
    /// Alloue et initialise une nouvelle pile kernel.
    pub fn new() -> Self {
        Self {
            canary: STACK_CANARY,
             [0u8; KERNEL_STACK_SIZE],
        }
    }

    /// Vérifie l'intégrité du canary.
    /// Doit être appelé en sortie de fonctions critiques.
    ///
    /// # Panics
    /// Panic si le canary a été modifié (stack overflow détecté).
    #[inline(always)]
    pub fn check_canary(&self) {
        if self.canary != STACK_CANARY {
            panic!(
                "STACK OVERFLOW DETECTED: canary corrompu ({:#x} != {:#x})",
                self.canary, STACK_CANARY
            );
        }
    }

    /// Retourne le pointeur vers le bas de pile (pour initialisation TSS).
    pub fn top(&mut self) -> u64 {
        let ptr = self.data.as_mut_ptr() as usize + KERNEL_STACK_SIZE;
        ptr as u64
    }
}

// ─── Macro pour vérification automatique ─────────────────────────────
/// Insère une vérification de canary en sortie de fonction.
///
/// Usage :
/// ```rust
/// fn ma_fonction_critique() -> Result<(), Error> {
///     stack_check!();
///     // ... code ...
/// }
/// ```
#[macro_export]
macro_rules! stack_check {
    () => {
        let _canary_guard = $crate::memory::stack::CanaryGuard::new();
    };
}

/// RAII guard pour vérification automatique de canary.
pub struct CanaryGuard {
    // Phantom data pour tie lifetime
    _marker: core::marker::PhantomData<*const ()>,
}

impl CanaryGuard {
    pub fn new() -> Self {
        Self { _marker: core::marker::PhantomData }
    }
}

impl Drop for CanaryGuard {
    fn drop(&mut self) {
        // Vérification automatique à la destruction (sortie de scope)
        // Note : nécessite accès à la pile courante via thread-local
        if let Some(stack) = crate::thread::current_stack() {
            stack.check_canary();
        }
    }
}
```

---

### CORR-50 : fd_table mark_stale() au lieu de close()

**Fichier cible** : `servers/vfs_server/src/fd_table.rs` + `isolation.rs`

```rust
// servers/vfs_server/src/fd_table.rs
// ✅ CORR-50 — État STALE pour fds invalidés post-restore

use core::sync::atomic::{AtomicU8, Ordering};

/// État d'un fd ouvert.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FdState {
    /// Fd opérationnel.
    Active = 0,
    /// Fd invalidé post-restore Phoenix (ObjectId disparu du disque).
    /// Les opérations retournent EIO. Les waiters sont réveillés.
    Stale = 1,
    /// Fd explicitement fermé par l'application.
    Closed = 2,
}

/// Entrée dans la table des fd.
pub struct FdEntry {
    pub obj_id: ObjectId,
    pub flags: u32,
    pub state: AtomicU8, // FdState encoded as u8
    pub wait_queue: WaitQueue,
}

impl FdEntry {
    pub fn new(obj_id: ObjectId, flags: u32) -> Self {
        Self {
            obj_id,
            flags,
            state: AtomicU8::new(FdState::Active as u8),
            wait_queue: WaitQueue::new(),
        }
    }
}

/// Marque atomiquement un fd comme STALE et réveille tous les waiters.
///
/// CORR-50 : Préféré à close() pendant restore Phoenix car :
///   - close() abrupte peut deadlocker les threads bloqués sur le fd
///   - mark_stale() notifie proprement avec EIO (waiters se réveillent)
///   - L'application peut détecter et gérer l'état STALE explicitement
///
/// Les opérations sur un fd STALE retournent EIO.
/// Un fd STALE peut être fermé normalement par l'application ensuite.
pub fn mark_stale(fd: u32) {
    if let Some(entry) = get_entry(fd) {
        let old_state = entry.state.swap(FdState::Stale as u8, Ordering::Release);

        if old_state == FdState::Closed as u8 {
            // Déjà fermé — rien à faire
            return;
        }

        // Réveiller tous les waiters (poll, read, write bloqués)
        entry.wait_queue.wake_all(WakeReason::Stale);

        log::debug!("fd {} marqué STALE (était {:?})", fd, FdState::from(old_state));
    }
}

// ─── Opérations sur fd vérifient état STALE ─────────────────────────
pub fn sys_exofs_read(fd: u32, buf: u64, len: usize) -> Result<usize, ExofsError> {
    let entry = get_entry(fd).ok_or(ExofsError::BadFd)?;

    // ✅ CORR-50 : vérifier état STALE avant toute opération
    let state = entry.state.load(Ordering::Acquire);
    if state == FdState::Stale as u8 {
        return Err(ExofsError::Io); // EIO — indique fd invalidé post-restore
    }
    if state == FdState::Closed as u8 {
        return Err(ExofsError::BadFd);
    }

    // ... suite normale de read ...
}
```

```rust
// servers/vfs_server/src/isolation.rs
// ✅ CORR-50 — Validation fd_table post-restore avec mark_stale()

pub fn validate_fd_table_after_restore() {
    log::info!("vfs_server: validation fd_table post-restore (mark_stale)");
    let mut stale_count: u32 = 0;

    for entry in fd_table::iter_open_fds() {
        // Vérifier existence de l'ObjectId dans ExoFS
        let exists = syscall::exofs_stat(entry.obj_id).is_ok();

        if !exists {
            log::warn!(
                "vfs_server: fd {} ObjectId {:?} invalide post-restore → STALE",
                entry.fd, entry.obj_id
            );

            // ✅ CORR-50 : mark_stale au lieu de close()
            // Notifie les threads bloqués avec EIO — pas de deadlock
            fd_table::mark_stale(entry.fd);
            stale_count += 1;
        }
    }

    log::info!(
        "vfs_server: {} fds marqués STALE post-restore (sur {} total)",
        stale_count,
        fd_table::count_open_fds()
    );

    // Les fds STALE seront fermés explicitement par les applications
    // qui recevront EIO sur leur prochain appel système
}
```

---

### CORR-51 : IRQ handlers — Purge PIDs morts

**Fichier cible** : `kernel/src/arch/x86_64/irq/routing.rs`

```rust
// kernel/src/arch/x86_64/irq/routing.rs
// ✅ CORR-51 — Purge automatique des handlers orphelins (PIDs morts)

use crate::process;
use heapless::Vec as HeaplessVec;

pub const MAX_HANDLERS_PER_IRQ: usize = 8;

/// Enregistre un handler pour une IRQ.
///
/// # Corrections appliquées
/// * CORR-37 : Vérification limite avant ajout
/// * CORR-51 : Purge PIDs morts avant test limite
/// * CORR-44 : Vérification vecteurs réservés
pub fn sys_irq_register(
    irq: u8,
    endpoint: IpcEndpoint,
    source_kind: IrqSourceKind,
    bdf: Option<PciBdf>,
) -> Result<u64, IrqError> {
    // ✅ CORR-44 : Vecteur réservé check
    if irq >= VECTOR_RESERVED_START {
        return Err(IrqError::VectorReserved);
    }

    let _irq_guard = arch::irq_save();
    let mut table = IRQ_TABLE.write();

    let route = table[irq as usize]
        .get_or_insert_with(|| IrqRoute::new(irq, source_kind));

    // ✅ CORR-51 : Purger les handlers de PIDs morts avant le test de limite.
    // Cas : driver crashe brutalement avant que do_exit() ne révoque ses handlers.
    // process::is_alive(pid) = false si le PID est terminé ou inexistant.
    // Cette purge est idempotente et sans effet si do_exit() a déjà nettoyé.
    route.handlers.retain(|h| {
        let alive = process::is_alive(h.owner_pid);
        if !alive {
            log::debug!(
                "sys_irq_register IRQ {}: purge handler orphelin PID {} (mort)",
                irq, h.owner_pid
            );
        }
        alive
    });

    // Vérification du kind (FIX-67 v7)
    if !route.handlers.is_empty() && route.source_kind != source_kind {
        return Err(IrqError::KindMismatch {
            existing: route.source_kind,
            requested: source_kind,
        });
    }

    // ✅ CORR-37 : Test de limite APRÈS purge
    if route.handlers.len() >= MAX_HANDLERS_PER_IRQ {
        log::error!(
            "sys_irq_register IRQ {}: limite {} handlers atteinte après purge — refus",
            irq, MAX_HANDLERS_PER_IRQ
        );
        return Err(IrqError::HandlerLimitReached);
    }

    // Vérifier doublon pour ce PID (un driver ne peut register qu'une fois par IRQ)
    route.handlers.retain(|h| h.owner_pid != current_process::pid());

    // Re-vérifier limite après retain (cas edge)
    if route.handlers.len() >= MAX_HANDLERS_PER_IRQ {
        return Err(IrqError::HandlerLimitReached);
    }

    // Ajouter nouveau handler
    let generation = GLOBAL_GEN.fetch_add(1, Ordering::Relaxed);
    let reg_id = new_reg_id();

    route.handlers.push(IrqHandler {
        reg_id,
        generation,
        owner_pid: current_process::pid(),
        endpoint,
    }).map_err(|_| IrqError::HandlerLimitReached)?;

    Ok(reg_id)
}
```

```rust
// kernel/src/process/registry.rs
// Nouvelle fonction pour CORR-51

/// Vérifie si un PID correspond à un processus actuellement actif.
/// Retourne false si le processus est terminé, zombie, ou inexistant.
/// Thread-safe (lecture atomique de la table des processus).
pub fn is_alive(pid: u32) -> bool {
    PROCESS_TABLE.read()
        .get(&pid)
        .map(|p| p.state != ProcessState::Dead && p.state != ProcessState::Zombie)
        .unwrap_or(false)
}
```

---

## 🔵 P3 — MINEUR (Nettoyage cosmétique)

### CORR-27 : MAX_CPUS → MAX_CORES

```rust
// ❌ AVANT — Constante locale incorrecte
const MAX_CPUS: usize = 64;

// ✅ APRÈS — Utiliser constante globale
use crate::arch::MAX_CORES; // = 256

// Remplacer toutes les occurrences
for cpu_id in 0..MAX_CPUS {  // ← changer
for core_id in 0..MAX_CORES { // ← ceci
```

---

### CORR-29 : user_gs_base → gs_base_user

```rust
// ❌ AVANT — Nommage incohérent
pub struct ThreadContext {
    pub fs_base_user: u64,
    pub user_gs_base: u64, // ← incohérent
}

// ✅ APRÈS — Cohérence nommage
pub struct ThreadContext {
    pub fs_base_user: u64,
    pub gs_base_user: u64, // ← cohérent
}
```

---

### CORR-30 : FixedString len: u32

```rust
// ❌ AVANT — usize (64-bit sur x86_64)
pub struct FixedString<const N: usize> {
     [u8; N],
    len: usize,
}

// ✅ APRÈS — u32 suffit pour N <= 4GB
pub struct FixedString<const N: usize> {
     [u8; N],
    len: u32,
}

// Assertion compile-time
const _: () = assert!(N <= 0xFFFF_FFFF); // Sanity check
```

---

## 📋 Checklist validation P2/P3

- [ ] **CORR-45** : IoVec align(8) + tests unitaires
- [ ] **CORR-46** : TL-38 documenté + vérifications direct_io
- [ ] **CORR-47** : quota::check_and_reserve() dans copy_file_range
- [ ] **CORR-48** : Stack canaries implémentés + macro stack_check!
- [ ] **CORR-50** : mark_stale() remplace close() post-restore
- [ ] **CORR-51** : process::is_alive() + purge IRQ handlers
- [ ] **CORR-27** : MAX_CORES harmonisé
- [ ] **CORR-29** : gs_base_user renommé
- [ ] **CORR-30** : FixedString len: u32

---

*Spécifications techniques P2/P3 — Prêtes à implémenter*
*Dernière mise à jour : Avril 2026*