//! # syscall/validation.rs — Validation des arguments userspace
//!
//! Fournit les primitives de validation et de copie mémoire
//! sécurisée entre l'espace utilisateur et le kernel.
//!
//! ## Modèle de sécurité
//!
//! Toute donnée provenant de Ring 3 est **non fiable** jusqu'à validation.
//! Les règles sont :
//! 1. Pointeur non-NULL
//! 2. Dans la plage canonique userspace (< [`USER_ADDR_MAX`])
//! 3. Toute la plage [ptr, ptr+len) doit être en espace utilisateur
//! 4. Pas de débordement arithmétique sur `ptr + len`
//! 5. Alignement correct pour les types non-packed
//!
//! ## copy_from_user / copy_to_user
//! Les fonctions ne déréférencent jamais directement l'adresse Ring 3 en
//! contexte noyau. Sur la cible kernel, elles traduisent l'adresse utilisateur
//! via l'espace d'adressage courant, déclenchent au besoin le même chemin de
//! demand-paging/CoW qu'un #PF userspace, puis copient via la physmap kernel.
//!
//! ## RÈGLE CONTRAT UNSAFE (regle_bonus.md)
//! Tout bloc `unsafe {}` est précédé d'un commentaire `// SAFETY:`.

use core::fmt;
use core::mem;
use core::sync::atomic::{AtomicU64, Ordering};

use alloc::vec::Vec;

use super::numbers::{E2BIG, EFAULT, EINVAL, ENOMEM};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de plage d'adresses
// ─────────────────────────────────────────────────────────────────────────────

/// Début du kernel en adresse virtuelle (x86_64 haut canonique)
/// Toute adresse ≥ USER_ADDR_MAX appartient au kernel.
pub const USER_ADDR_MAX: u64 = 0x0000_8000_0000_0000;

/// Longueur maximale d'un chemin de fichier
pub const PATH_MAX: usize = 4096;

/// Longueur maximale d'une chaîne générique passée en syscall
pub const STRING_MAX: usize = 65536;

/// Taille maximale d'un buffer I/O en un seul syscall (256 MiB)
pub const IO_BUF_MAX: usize = 256 * 1024 * 1024;

/// Nombre maximal de paramètres argv/envp dans execve
pub const ARGV_MAX: usize = 1024;

// ─────────────────────────────────────────────────────────────────────────────
// Compteurs d'instrumentation
// ─────────────────────────────────────────────────────────────────────────────

static VALIDATION_COUNT: AtomicU64 = AtomicU64::new(0);
static VALIDATION_FAULT_COUNT: AtomicU64 = AtomicU64::new(0);
static COPY_FROM_USER_BYTES: AtomicU64 = AtomicU64::new(0);
static COPY_TO_USER_BYTES: AtomicU64 = AtomicU64::new(0);

/// Retourne le nombre total de validations effectuées
#[inline]
pub fn validation_count() -> u64 {
    VALIDATION_COUNT.load(Ordering::Relaxed)
}
/// Retourne le nombre de fautes de validation (EFAULT levé)
#[inline]
pub fn validation_fault_count() -> u64 {
    VALIDATION_FAULT_COUNT.load(Ordering::Relaxed)
}
/// Retourne le total d'octets copiés depuis userspace
#[inline]
pub fn copy_from_user_bytes_total() -> u64 {
    COPY_FROM_USER_BYTES.load(Ordering::Relaxed)
}
/// Retourne le total d'octets copiés vers userspace
#[inline]
pub fn copy_to_user_bytes_total() -> u64 {
    COPY_TO_USER_BYTES.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// UserPtr<T> — pointeur typé provenant de l'espace utilisateur
// ─────────────────────────────────────────────────────────────────────────────

/// Wrapper de type autour d'un pointeur brut provenant de Ring 3.
///
/// Non déréférençable directement. Doit être validé via
/// [`UserPtr::validate`] avant tout accès, ce qui retourne un
/// [`ValidatedUserPtr<T>`].
#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct UserPtr<T> {
    addr: u64,
    _phantom: core::marker::PhantomData<*mut T>,
}

// SAFETY: UserPtr est uniquement un entier encodant une adresse Ring 3.
// Il n'est pas déréférencé directement, donc Send+Sync sont sûrs.
unsafe impl<T> Send for UserPtr<T> {}
unsafe impl<T> Sync for UserPtr<T> {}

impl<T> UserPtr<T> {
    /// Construit un `UserPtr` depuis une adresse brute (issue d'un registre syscall).
    #[inline]
    pub fn from_raw(addr: u64) -> Self {
        Self {
            addr,
            _phantom: core::marker::PhantomData,
        }
    }

    /// Retourne l'adresse brute sans validation.
    #[inline]
    pub fn as_raw(&self) -> u64 {
        self.addr
    }

    /// Retourne `true` si le pointeur est nul.
    #[inline]
    pub fn is_null(&self) -> bool {
        self.addr == 0
    }

    /// Valide le pointeur et retourne un [`ValidatedUserPtr<T>`].
    ///
    /// Vérifie :
    /// - Non-NULL
    /// - Dans la plage canonique userspace
    /// - Aligné sur `align_of::<T>()`
    /// - La plage `[addr, addr + size_of::<T>())` ne dépasse pas `USER_ADDR_MAX`
    pub fn validate(&self) -> Result<ValidatedUserPtr<T>, SyscallError> {
        VALIDATION_COUNT.fetch_add(1, Ordering::Relaxed);
        validate_user_range(self.addr, mem::size_of::<T>(), mem::align_of::<T>())?;
        Ok(ValidatedUserPtr {
            addr: self.addr,
            _phantom: core::marker::PhantomData,
        })
    }

    /// Valide un pointeur nullable (`NULL` est accepté → retourne `None`).
    pub fn validate_nullable(&self) -> Result<Option<ValidatedUserPtr<T>>, SyscallError> {
        if self.addr == 0 {
            return Ok(None);
        }
        self.validate().map(Some)
    }
}

impl<T> fmt::Debug for UserPtr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UserPtr(0x{:016x})", self.addr)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ValidatedUserPtr<T> — pointeur validé, déréférençable sous unsafe
// ─────────────────────────────────────────────────────────────────────────────

/// Pointeur utilisateur dont la plage et l'alignement ont été vérifiés.
/// Peut être copié via `copy_from_user` / `copy_to_user`.
pub struct ValidatedUserPtr<T> {
    addr: u64,
    _phantom: core::marker::PhantomData<*mut T>,
}

impl<T: Copy> ValidatedUserPtr<T> {
    /// Copie la valeur T depuis l'espace utilisateur.
    ///
    /// Utilise des lectures volatiles pour éviter les optimisations compilateur.
    ///
    /// # Errors
    /// Retourne [`SyscallError::Fault`] si un page fault se produit
    /// (dans un kernel complet, géré via la table de fixups).
    pub fn read(&self) -> Result<T, SyscallError> {
        let mut value = mem::MaybeUninit::<T>::uninit();
        copy_from_user(
            value.as_mut_ptr() as *mut u8,
            self.addr as *const u8,
            mem::size_of::<T>(),
        )?;
        // SAFETY: copy_from_user a rempli tous les octets de value si Ok.
        Ok(unsafe { value.assume_init() })
    }

    /// Écrit une valeur T vers l'espace utilisateur.
    pub fn write(&self, value: T) -> Result<(), SyscallError> {
        copy_to_user(
            self.addr as *mut u8,
            &value as *const T as *const u8,
            mem::size_of::<T>(),
        )
    }
}

impl<T> ValidatedUserPtr<T> {
    /// Retourne l'adresse validée.
    #[inline]
    pub fn as_raw(&self) -> u64 {
        self.addr
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UserBuf — slice d'octets userspace validée
// ─────────────────────────────────────────────────────────────────────────────

/// Représente un buffer utilisateur (adresse + longueur) entièrement validé.
pub struct UserBuf {
    addr: u64,
    len: usize,
}

impl UserBuf {
    /// Valide un buffer (adresse + longueur) en espace utilisateur.
    ///
    /// - `ptr` : adresse de départ (issue d'un registre syscall)
    /// - `len` : taille en octets
    /// - `max` : limite supérieure autorisée (`IO_BUF_MAX`, `PATH_MAX`, etc.)
    pub fn validate(ptr: u64, len: usize, max: usize) -> Result<Self, SyscallError> {
        VALIDATION_COUNT.fetch_add(1, Ordering::Relaxed);
        if len > max {
            record_fault();
            return Err(SyscallError::TooBig);
        }
        validate_user_range(ptr, len, 1)?;
        Ok(Self { addr: ptr, len })
    }

    /// Copie le contenu du buffer utilisateur dans `dst`.
    ///
    /// `dst` doit avoir une longueur exactement égale à `self.len`.
    pub fn read_into(&self, dst: &mut [u8]) -> Result<(), SyscallError> {
        if dst.len() != self.len {
            return Err(SyscallError::Invalid);
        }
        copy_from_user(dst.as_mut_ptr(), self.addr as *const u8, self.len)
    }

    /// Copie `src` dans le buffer utilisateur.
    pub fn write_from(&self, src: &[u8]) -> Result<(), SyscallError> {
        if src.len() > self.len {
            return Err(SyscallError::TooBig);
        }
        copy_to_user(self.addr as *mut u8, src.as_ptr(), src.len())
    }

    /// Retourne la longueur validée.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }
    /// Retourne true si le buffer est vide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Retourne l'adresse validée.
    #[inline]
    pub fn addr(&self) -> u64 {
        self.addr
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UserStr — chaîne C null-terminée depuis userspace
// ─────────────────────────────────────────────────────────────────────────────

/// Représente une chaîne C null-terminée validée depuis l'espace utilisateur.
///
/// Le contenu est copié dans un buffer kernel interne lors de la validation.
pub struct UserStr {
    /// Buffer kernel contenant la chaîne UTF-8 copiée (sans le '\0').
    ///
    /// Stockage heap: les chemins/argv/envp ne doivent jamais materialiser un
    /// buffer PATH_MAX/STRING_MAX dans une frame de syscall.
    buf: Vec<u8>,
}

impl UserStr {
    /// Lit et valide une chaîne C null-terminée depuis userspace.
    ///
    /// - `ptr` : adresse de la chaîne (issue d'un registre syscall)
    /// - `max` : longueur maximale tolérée (sans '\0')
    ///
    /// # Errors
    /// - [`SyscallError::Fault`]   si `ptr` est invalide
    /// - [`SyscallError::TooBig`]  si aucun '\0' avant `max` octets
    /// - [`SyscallError::Invalid`] si la chaîne contient des octets invalides
    pub fn from_user(ptr: u64, max: usize) -> Result<Self, SyscallError> {
        VALIDATION_COUNT.fetch_add(1, Ordering::Relaxed);
        if ptr == 0 {
            record_fault();
            return Err(SyscallError::Fault);
        }
        // Valider que l'adresse de départ est bien en userspace
        if ptr >= USER_ADDR_MAX {
            record_fault();
            return Err(SyscallError::Fault);
        }
        let capped_max = max.min(STRING_MAX);
        let mut buf = Vec::new();
        buf.try_reserve(capped_max)
            .map_err(|_| SyscallError::NoMemory)?;

        // Copie octet par octet jusqu'au '\0' ou ptr+max
        // (safe car on vérifie la borne userspace ci-dessous)
        let mut offset = 0usize;
        loop {
            if offset >= capped_max {
                record_fault();
                return Err(SyscallError::TooBig);
            }
            let byte_addr = ptr.checked_add(offset as u64).ok_or(SyscallError::Fault)?;
            if byte_addr >= USER_ADDR_MAX {
                record_fault();
                return Err(SyscallError::Fault);
            }
            let mut byte = 0u8;
            copy_from_user(&mut byte as *mut u8, byte_addr as *const u8, 1)?;
            if byte == 0 {
                break;
            }
            buf.push(byte);
            offset += 1;
        }
        Ok(Self { buf })
    }

    /// Retourne la chaîne comme slice d'octets (sans le null terminal).
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Retourne la longueur sans le null-terminal.
    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Retourne true si la chaîne est vide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Tente de convertir en &str (UTF-8 strict).
    pub fn as_str(&self) -> Result<&str, SyscallError> {
        core::str::from_utf8(&self.buf).map_err(|_| SyscallError::Invalid)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SyscallError — erreurs de validation remontées vers dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs retournées par les fonctions de validation.
/// Converties en codes errno Linux par [`SyscallError::to_errno`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SyscallError {
    /// Adresse utilisateur invalide (NULL, hors plage, ou non mappée)
    Fault,
    /// Argument numérique invalide (valeur hors plage, flag inconnu, etc.)
    Invalid,
    /// Taille ou longueur dépasse la limite autorisée
    TooBig,
    /// Accès refusé (capability ou permission manquante)
    Access,
    /// Ressource non trouvée
    NotFound,
    /// Ressource occupée ou verrou non disponible
    Busy,
    /// Interruption par signal
    Interrupted,
    /// Memoire insuffisante pendant une copie/validation kernel
    NoMemory,
    /// Opération non supportée
    NotSupported,
}

impl SyscallError {
    /// Convertit en code errno Linux (valeur négative).
    #[inline]
    pub const fn to_errno(self) -> i64 {
        match self {
            SyscallError::Fault => EFAULT,
            SyscallError::Invalid => EINVAL,
            SyscallError::TooBig => E2BIG,
            SyscallError::Access => -13,     // EACCES
            SyscallError::NotFound => -2,    // ENOENT
            SyscallError::Busy => -16,       // EBUSY
            SyscallError::Interrupted => -4, // EINTR
            SyscallError::NoMemory => ENOMEM,
            SyscallError::NotSupported => -38, // ENOSYS
        }
    }
}

impl fmt::Display for SyscallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SyscallError::Fault => "bad address (EFAULT)",
            SyscallError::Invalid => "invalid argument (EINVAL)",
            SyscallError::TooBig => "argument too large (E2BIG)",
            SyscallError::Access => "permission denied (EACCES)",
            SyscallError::NotFound => "not found (ENOENT)",
            SyscallError::Busy => "resource busy (EBUSY)",
            SyscallError::Interrupted => "interrupted (EINTR)",
            SyscallError::NoMemory => "out of memory (ENOMEM)",
            SyscallError::NotSupported => "not supported (ENOSYS)",
        };
        f.write_str(s)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Primitives internes de validation
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie qu'un intervalle [addr, addr+len) est entièrement en userspace.
///
/// Conditions vérifiées :
/// 1. `addr` non-NULL (sauf si `len == 0`)
/// 2. `addr < USER_ADDR_MAX`
/// 3. `addr + len ≤ USER_ADDR_MAX` (pas de débordement, pas de chevauchement kernel)
/// 4. Alignement sur `align` (doit être une puissance de 2)
#[inline]
fn validate_user_range(addr: u64, len: usize, align: usize) -> Result<(), SyscallError> {
    // Cas spécial longueur nulle : seul NULL refusé pour cohérence POSIX
    if len == 0 {
        return Ok(());
    }
    // Null pointer
    if addr == 0 {
        record_fault();
        return Err(SyscallError::Fault);
    }
    // Début dans le kernel
    if addr >= USER_ADDR_MAX {
        record_fault();
        return Err(SyscallError::Fault);
    }
    // Vérification anti-wrap : addr + len ne doit pas déborder u64 ni franchir USER_ADDR_MAX
    let end = addr.checked_add(len as u64).ok_or_else(|| {
        record_fault();
        SyscallError::Fault
    })?;
    if end > USER_ADDR_MAX {
        record_fault();
        return Err(SyscallError::Fault);
    }
    // Vérification alignement (align doit être puissance de 2)
    debug_assert!(
        align.is_power_of_two(),
        "align doit être une puissance de 2"
    );
    if align > 1 && (addr as usize) % align != 0 {
        record_fault();
        return Err(SyscallError::Fault);
    }
    Ok(())
}

#[cfg(target_os = "none")]
fn current_user_address_space() -> Option<&'static crate::memory::virt::UserAddressSpace> {
    let tcb_raw = unsafe { crate::arch::x86_64::smp::percpu::read_current_tcb() };
    if tcb_raw == 0 {
        return None;
    }

    let tcb = unsafe { &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock) };
    let pid = crate::process::core::pid::Pid(tcb.pid.0);
    let pcb = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid)?;
    let as_ptr = pcb.address_space_ptr();
    if as_ptr.is_null() {
        return None;
    }

    // SAFETY: l'address space appartient au PCB vivant du thread courant.
    Some(unsafe { &*(as_ptr as *const crate::memory::virt::UserAddressSpace) })
}

#[cfg(target_os = "none")]
fn walk_user_mapping(
    user_as: &crate::memory::virt::UserAddressSpace,
    addr: u64,
) -> Option<(
    crate::memory::core::PhysAddr,
    crate::memory::virt::page_table::PageTableEntry,
)> {
    use crate::memory::core::{PhysAddr, VirtAddr, PAGE_SIZE};
    use crate::memory::virt::page_table::{PageTableWalker, WalkResult};

    let virt = VirtAddr::new(addr);
    let walker = PageTableWalker::new(user_as.pml4_phys());
    match walker.walk_read(virt) {
        WalkResult::Leaf { entry, .. } => {
            let off = addr & (PAGE_SIZE as u64 - 1);
            Some((PhysAddr::new(entry.phys_addr().as_u64() + off), entry))
        }
        WalkResult::HugePage { entry, level } => {
            let page_size = level.page_size() as u64;
            let off = addr & (page_size - 1);
            Some((PhysAddr::new(entry.phys_addr().as_u64() + off), entry))
        }
        _ => None,
    }
}

#[cfg(target_os = "none")]
fn resolve_user_page(
    user_as: &crate::memory::virt::UserAddressSpace,
    addr: u64,
    write: bool,
) -> Result<crate::memory::core::PhysAddr, SyscallError> {
    use crate::arch::x86_64::memory_iface::UserFaultAllocator;
    use crate::memory::core::{VirtAddr, PAGE_SIZE};
    use crate::memory::virt::fault::{handle_page_fault, FaultCause, FaultContext, FaultResult};

    if let Some((phys, entry)) = walk_user_mapping(user_as, addr) {
        if !entry.is_user() {
            record_fault();
            return Err(SyscallError::Fault);
        }
        if !write || (entry.is_writable() && !entry.is_cow()) {
            return Ok(phys);
        }
    }

    let page_addr = VirtAddr::new(addr & !(PAGE_SIZE as u64 - 1));
    let vma = match user_as.find_vma(page_addr) {
        Some(vma) => vma,
        None => {
            record_fault();
            return Err(SyscallError::Fault);
        }
    };
    let cause = if write {
        FaultCause::Write
    } else {
        FaultCause::Read
    };
    let ctx = FaultContext::new(page_addr, cause, false).with_vma(vma);
    let alloc = UserFaultAllocator::new(user_as);
    match handle_page_fault(&ctx, &alloc) {
        FaultResult::Handled => {}
        _ => {
            record_fault();
            return Err(SyscallError::Fault);
        }
    }

    if let Some((phys, entry)) = walk_user_mapping(user_as, addr) {
        if entry.is_user() && (!write || (entry.is_writable() && !entry.is_cow())) {
            return Ok(phys);
        }
    }

    record_fault();
    Err(SyscallError::Fault)
}

#[cfg(target_os = "none")]
fn copy_from_user_resolved(dst: *mut u8, src: *const u8, len: usize) -> Result<(), SyscallError> {
    use crate::memory::core::{phys_to_virt, PAGE_SIZE};

    let user_as = current_user_address_space().ok_or_else(|| {
        record_fault();
        SyscallError::Fault
    })?;
    let mut copied = 0usize;
    while copied < len {
        let user_addr = (src as u64).saturating_add(copied as u64);
        let page_off = (user_addr as usize) & (PAGE_SIZE - 1);
        let chunk = core::cmp::min(PAGE_SIZE - page_off, len - copied);
        let phys = resolve_user_page(user_as, user_addr, false)?;
        let kernel_src = phys_to_virt(phys).as_u64() as *const u8;
        // SAFETY: resolve_user_page garantit que la source est l'alias physmap
        // de la page utilisateur courante; dst couvre len octets kernel.
        unsafe {
            core::ptr::copy_nonoverlapping(kernel_src, dst.add(copied), chunk);
        }
        copied += chunk;
    }
    Ok(())
}

#[cfg(target_os = "none")]
fn copy_to_user_resolved(dst: *mut u8, src: *const u8, len: usize) -> Result<(), SyscallError> {
    use crate::memory::core::{phys_to_virt, PAGE_SIZE};

    let user_as = current_user_address_space().ok_or_else(|| {
        record_fault();
        SyscallError::Fault
    })?;
    let mut copied = 0usize;
    while copied < len {
        let user_addr = (dst as u64).saturating_add(copied as u64);
        let page_off = (user_addr as usize) & (PAGE_SIZE - 1);
        let chunk = core::cmp::min(PAGE_SIZE - page_off, len - copied);
        let phys = resolve_user_page(user_as, user_addr, true)?;
        let kernel_dst = phys_to_virt(phys).as_u64() as *mut u8;
        // SAFETY: resolve_user_page a validé/résolu une page userspace writable
        // et retourne son alias physmap kernel; src couvre len octets kernel.
        unsafe {
            core::ptr::copy_nonoverlapping(src.add(copied), kernel_dst, chunk);
        }
        copied += chunk;
    }
    Ok(())
}

/// Copie `len` octets depuis `src` (userspace) vers `dst` (kernel).
///
/// # Safety
/// `dst` doit pointer vers un buffer kernel valide de longueur >= `len`.
pub fn copy_from_user(dst: *mut u8, src: *const u8, len: usize) -> Result<(), SyscallError> {
    if len == 0 {
        return Ok(());
    }
    validate_user_range(src as u64, len, 1)?;
    #[cfg(target_os = "none")]
    {
        copy_from_user_resolved(dst, src, len)?;
    }
    #[cfg(not(target_os = "none"))]
    unsafe {
        for i in 0..len {
            let byte = core::ptr::read_volatile(src.add(i));
            core::ptr::write(dst.add(i), byte);
        }
    }
    COPY_FROM_USER_BYTES.fetch_add(len as u64, Ordering::Relaxed);
    Ok(())
}

/// Copie `len` octets depuis `src` (kernel) vers `dst` (userspace).
///
/// # Safety
/// `src` doit pointer vers `len` octets de données kernel valides.
pub fn copy_to_user(dst: *mut u8, src: *const u8, len: usize) -> Result<(), SyscallError> {
    if len == 0 {
        return Ok(());
    }
    validate_user_range(dst as u64, len, 1)?;
    #[cfg(target_os = "none")]
    {
        copy_to_user_resolved(dst, src, len)?;
    }
    #[cfg(not(target_os = "none"))]
    unsafe {
        for i in 0..len {
            let byte = core::ptr::read(src.add(i));
            core::ptr::write_volatile(dst.add(i), byte);
        }
    }
    COPY_TO_USER_BYTES.fetch_add(len as u64, Ordering::Relaxed);
    Ok(())
}

/// Incrément interne du compteur de fautes de validation.
#[inline(always)]
fn record_fault() {
    VALIDATION_FAULT_COUNT.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique de haut niveau — helpers directs pour les handlers syscall
// ─────────────────────────────────────────────────────────────────────────────

/// Valide et lit un `T` depuis un pointeur userspace.
///
/// Combine `UserPtr::validate()` et `ValidatedUserPtr::read()` en une seule
/// opération. Retourne l'errno en cas d'échec.
///
/// # Exemple
/// ```no_run
/// let timespec: Timespec = read_user_typed::<Timespec>(frame.rsi)?;
/// ```
pub fn read_user_typed<T: Copy>(ptr_raw: u64) -> Result<T, SyscallError> {
    UserPtr::<T>::from_raw(ptr_raw).validate()?.read()
}

/// Valide et écrit un `T` vers un pointeur userspace.
pub fn write_user_typed<T: Copy>(ptr_raw: u64, value: T) -> Result<(), SyscallError> {
    UserPtr::<T>::from_raw(ptr_raw).validate()?.write(value)
}

/// Valide un buffer `(ptr, len)` et copie les données dans un `Vec` kernel.
///
/// `max` est la limite de taille autorisée (ex: `IO_BUF_MAX` pour `read()`).
pub fn read_user_buf_to_vec(
    ptr: u64,
    len: usize,
    max: usize,
) -> Result<alloc::vec::Vec<u8>, SyscallError> {
    let buf = UserBuf::validate(ptr, len, max)?;
    let mut vec = alloc::vec![0u8; len];
    buf.read_into(&mut vec)?;
    Ok(vec)
}

/// Valide un chemin de fichier C null-terminé depuis userspace.
///
/// Longueur maximale : [`PATH_MAX`] (4096 octets).
pub fn read_user_path(ptr: u64) -> Result<UserStr, SyscallError> {
    UserStr::from_user(ptr, PATH_MAX)
}

/// Valide un argument entier comme descripteur de fichier (≥0 et <65536).
#[inline]
pub fn validate_fd(raw: u64) -> Result<i32, SyscallError> {
    if raw > 65535 {
        return Err(SyscallError::Invalid);
    }
    Ok(raw as i32)
}

/// Valide un ensemble de flags en vérifiant qu'aucun bit non supporté n'est levé.
#[inline]
pub fn validate_flags(raw: u64, allowed_mask: u64) -> Result<u64, SyscallError> {
    if raw & !allowed_mask != 0 {
        return Err(SyscallError::Invalid);
    }
    Ok(raw)
}

/// Valide un PID (doit être > 0 et < 4194304).
#[inline]
pub fn validate_pid(raw: u64) -> Result<u32, SyscallError> {
    if raw == 0 || raw >= 4194304 {
        return Err(SyscallError::Invalid);
    }
    Ok(raw as u32)
}

/// Valide un signal livrable (1..=63, POSIX + temps-reel).
///
/// `kill(pid, 0)` est une sonde d'existence : elle est traitee par le handler
/// `kill` avant cet appel, car le signal 0 n'est jamais livrable.
#[inline]
pub fn validate_signal(raw: u64) -> Result<u32, SyscallError> {
    if raw == 0 || raw > crate::process::signal::delivery::MAX_SIGNAL_NUMBER as u64 {
        return Err(SyscallError::Invalid);
    }
    Ok(raw as u32)
}

/// Valide un CLOCK_ID (valeurs POSIX standard).
#[inline]
pub fn validate_clockid(raw: u64) -> Result<u32, SyscallError> {
    // POSIX clock IDs : 0=REALTIME, 1=MONOTONIC, 2=PROCESS_CPUTIME_ID,
    // 3=THREAD_CPUTIME_ID, 4=MONOTONIC_RAW, 5=REALTIME_COARSE, 6=MONOTONIC_COARSE,
    // 7=BOOTTIME
    if raw > 7 {
        return Err(SyscallError::Invalid);
    }
    Ok(raw as u32)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests unitaires (cfg(test) → jamais compilés dans le kernel binaire)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_null_ptr() {
        let result = validate_user_range(0, 16, 1);
        assert!(matches!(result, Err(SyscallError::Fault)));
    }

    #[test]
    fn test_validate_kernel_ptr() {
        let kernel_addr = 0xFFFF_8000_0000_0000u64;
        let result = validate_user_range(kernel_addr, 8, 1);
        assert!(matches!(result, Err(SyscallError::Fault)));
    }

    #[test]
    fn test_validate_valid_range() {
        // Adresse canonique userspace valide
        let result = validate_user_range(0x0000_0000_0040_0000, 4096, 1);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_wrap_overflow() {
        // addr + len déborde u64
        let result = validate_user_range(u64::MAX - 4, 16, 1);
        assert!(matches!(result, Err(SyscallError::Fault)));
    }

    #[test]
    fn test_validate_end_past_usermax() {
        // addr valide mais s'étend dans le kernel
        let result = validate_user_range(USER_ADDR_MAX - 8, 16, 1);
        assert!(matches!(result, Err(SyscallError::Fault)));
    }

    #[test]
    fn test_validate_fd() {
        assert!(validate_fd(0).is_ok());
        assert!(validate_fd(65534).is_ok());
        assert!(matches!(validate_fd(65536), Err(SyscallError::Invalid)));
    }

    #[test]
    fn test_validate_pid() {
        assert!(matches!(validate_pid(0), Err(SyscallError::Invalid)));
        assert!(validate_pid(1).is_ok());
        assert!(validate_pid(4194303).is_ok());
        assert!(matches!(validate_pid(4194304), Err(SyscallError::Invalid)));
    }

    #[test]
    fn test_validate_signal() {
        assert!(matches!(validate_signal(0), Err(SyscallError::Invalid)));
        assert!(validate_signal(1).is_ok());
        assert!(validate_signal(63).is_ok());
        assert!(matches!(validate_signal(64), Err(SyscallError::Invalid)));
    }

    #[test]
    fn test_syscall_error_to_errno() {
        assert_eq!(SyscallError::Fault.to_errno(), EFAULT);
        assert_eq!(SyscallError::Invalid.to_errno(), EINVAL);
        assert_eq!(SyscallError::TooBig.to_errno(), E2BIG);
    }
}
