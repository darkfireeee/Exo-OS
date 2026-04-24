// kernel/src/process/thread/local_storage.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// TLS (Thread Local Storage) — gestion du segment GS pour x86_64
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture x86_64 :
//   Le segment GS est utilisé pour les données TLS.
//   GS.base (MSR_GS_BASE = 0xC0000101) = adresse du bloc TLS statique.
//   SWAPGS bascule entre le GS kernel (ptr CPU state) et le GS userspace (TLS).
//
// Implémentation simplifiée :
//   • TlsBlock : copie du .tdata/.tbss initialisé par execve() puis copié par clone().
//   • set_gs_base() : écrit ARCH_SET_GS via wrmsrl(MSR_FS_BASE, addr).
//   • TLS dynamique (dlopen / __tls_get_addr) : hors scope noyau.
// ═══════════════════════════════════════════════════════════════════════════════

use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

/// Taille maximale d'un bloc TLS statique (64 KiB).
pub const TLS_MAX_SIZE: usize = 65536;

/// Clé TLS dynamique (pthread_key_t).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct TlsKey(pub u32);

impl TlsKey {
    pub const INVALID: Self = Self(u32::MAX);
    pub fn is_valid(self) -> bool {
        self.0 != u32::MAX
    }
}

/// Bloc TLS statique d'un thread.
///
/// Contient la copie du .tdata initialement chargé par execve(),
/// suivie du .tbss (zéro-initialisé).
pub struct TlsBlock {
    /// Données TLS (tdata + tbss).
    data: Box<[u8]>,
    /// Taille du segment tdata.
    tdata_size: usize,
    /// Taille totale (tdata + tbss).
    total_size: usize,
    /// Adresse userspace de base du bloc TLS.
    user_base: u64,
}

impl TlsBlock {
    /// Crée un bloc TLS en copiant le modèle tdata fourni.
    pub fn new(tdata: &[u8], tbss_size: usize, user_base: u64) -> Option<Self> {
        let total = tdata.len() + tbss_size;
        if total == 0 || total > TLS_MAX_SIZE {
            return None;
        }
        let mut data = alloc::vec![0u8; total].into_boxed_slice();
        data[..tdata.len()].copy_from_slice(tdata);
        // tbss est déjà zéro par alloc::vec!
        Some(Self {
            data,
            tdata_size: tdata.len(),
            total_size: total,
            user_base,
        })
    }

    /// Pointeur vers les données (pour écriture de GS.base).
    #[inline(always)]
    pub fn as_ptr(&self) -> *const u8 {
        self.data.as_ptr()
    }

    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.data.as_mut_ptr()
    }

    /// Adresse userspace de ce bloc.
    #[inline(always)]
    pub fn user_base(&self) -> u64 {
        self.user_base
    }

    /// Taille totale en bytes.
    #[inline(always)]
    pub fn size(&self) -> usize {
        self.total_size
    }

    /// Clone le bloc TLS pour un nouveau thread (fork ou pthread_create).
    pub fn clone_for_thread(&self, new_user_base: u64) -> Option<Self> {
        let data = self.data.to_vec().into_boxed_slice();
        Some(Self {
            data,
            tdata_size: self.tdata_size,
            total_size: self.total_size,
            user_base: new_user_base,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TlsRegistry — registre global des clés TLS dynamiques
// ─────────────────────────────────────────────────────────────────────────────

const MAX_TLS_KEYS: usize = 1024;

/// Entrée de la table des clés TLS dynamiques.
struct TlsKeyEntry {
    used: bool,
    destructor: Option<u64>, // pointeur userspace vers la fonction destructeur
}

/// Registre global des clés TLS dynamiques (pthread_key_create/destroy).
pub struct TlsRegistry {
    keys: UnsafeCell<[TlsKeyEntry; MAX_TLS_KEYS]>,
    n_used: AtomicU32,
    lock: crate::scheduler::sync::spinlock::SpinLock<()>,
}

// SAFETY: TlsRegistry est accédé via SpinLock.
unsafe impl Sync for TlsRegistry {}

pub static TLS_REGISTRY: TlsRegistry = TlsRegistry {
    // SAFETY: TlsKeyEntry n'est pas Copy, initialiser manuellement pour les 1024 entrées.
    // Option: utiliser une construction const via const fn.
    keys: UnsafeCell::new(
        [const {
            TlsKeyEntry {
                used: false,
                destructor: None,
            }
        }; MAX_TLS_KEYS],
    ),
    n_used: AtomicU32::new(0),
    lock: crate::scheduler::sync::spinlock::SpinLock::new(()),
};

impl TlsRegistry {
    /// Alloue une nouvelle clé TLS dynamique (pthread_key_create).
    pub fn alloc_key(&self, destructor: Option<u64>) -> Option<TlsKey> {
        let _g = self.lock.lock();
        // SAFETY: accès sous spinlock garantis exclusifs.
        let keys = unsafe { &mut *self.keys.get() };
        for (i, entry) in keys.iter_mut().enumerate() {
            if !entry.used {
                entry.used = true;
                entry.destructor = destructor;
                self.n_used.fetch_add(1, Ordering::Relaxed);
                return Some(TlsKey(i as u32));
            }
        }
        None // EAGAIN
    }

    /// Libère une clé TLS (pthread_key_delete).
    pub fn free_key(&self, key: TlsKey) -> bool {
        if key.0 as usize >= MAX_TLS_KEYS {
            return false;
        }
        let _g = self.lock.lock();
        // SAFETY: accès sous spinlock.
        let entry = unsafe { &mut (*self.keys.get())[key.0 as usize] };
        if entry.used {
            entry.used = false;
            entry.destructor = None;
            self.n_used.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Retourne le destructeur associé à une clé (None si inexistant).
    pub fn get_destructor(&self, key: TlsKey) -> Option<u64> {
        if key.0 as usize >= MAX_TLS_KEYS {
            return None;
        }
        // SAFETY: lecture légère sans lock (conservative).
        let entry = unsafe { &(*self.keys.get())[key.0 as usize] };
        if entry.used {
            entry.destructor
        } else {
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GS.base — MSR ARCH_SET_GS x86_64
// ─────────────────────────────────────────────────────────────────────────────

/// MSR_FS_BASE (0xC0000100) = base du segment FS (libc utilise FS pour TLS).
const MSR_FS_BASE: u32 = 0xC0000100;
/// MSR_GS_BASE (0xC0000101) = base du segment GS.
#[allow(dead_code)]
const MSR_GS_BASE: u32 = 0xC0000101;
/// MSR_KERNEL_GS_BASE (0xC0000102) = GS backup (swapgs).
#[allow(dead_code)]
const MSR_KERNEL_GS_BASE: u32 = 0xC0000102;

/// Écrit MSR_FS_BASE avec l'adresse de base TLS (appelé lors du context switch).
///
/// # Safety
/// Doit être appelé depuis un contexte kernel (Ring 0) pendant le context switch.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn set_fs_base(addr: u64) {
    // SAFETY: instruction wrmsrl valide en Ring 0.
    core::arch::asm!(
        "wrmsr",
        in("ecx") MSR_FS_BASE,
        in("eax") addr as u32,
        in("edx") (addr >> 32) as u32,
        options(nostack, nomem),
    );
}

/// Lit MSR_FS_BASE.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn get_fs_base() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: instruction rdmsr valide en Ring 0.
    core::arch::asm!(
        "rdmsr",
        in("ecx") MSR_FS_BASE,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (hi as u64) << 32 | lo as u64
}
