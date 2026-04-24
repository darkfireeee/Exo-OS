// kernel/src/process/namespace/uts_ns.rs
//
// Espace de noms UTS (nom d'hôte) — Exo-OS Couche 1.5

use crate::scheduler::sync::spinlock::SpinLock;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

const HOSTNAME_MAX: usize = 64;
const DOMAINNAME_MAX: usize = 64;

/// Espace de noms UTS.
#[repr(C)]
pub struct UtsNamespace {
    pub id: u32,
    pub refcount: AtomicU32,
    pub valid: AtomicU32,
    lock: SpinLock<()>,
    hostname: UnsafeCell<[u8; HOSTNAME_MAX]>,
    hostname_len: AtomicU32,
    domainname: UnsafeCell<[u8; DOMAINNAME_MAX]>,
    domainname_len: AtomicU32,
}

impl UtsNamespace {
    const fn new_root() -> Self {
        let mut hostname = [0u8; HOSTNAME_MAX];
        // "exo-os" en ASCII
        hostname[0] = b'e';
        hostname[1] = b'x';
        hostname[2] = b'o';
        hostname[3] = b'-';
        hostname[4] = b'o';
        hostname[5] = b's';
        Self {
            id: 0,
            refcount: AtomicU32::new(1),
            valid: AtomicU32::new(1),
            lock: SpinLock::new(()),
            hostname: UnsafeCell::new(hostname),
            hostname_len: AtomicU32::new(6),
            domainname: UnsafeCell::new([0u8; DOMAINNAME_MAX]),
            domainname_len: AtomicU32::new(0),
        }
    }

    pub fn get_hostname(&self, buf: &mut [u8]) -> usize {
        let _g = self.lock.lock();
        let len = (self.hostname_len.load(Ordering::Acquire) as usize)
            .min(buf.len())
            .min(HOSTNAME_MAX);
        // SAFETY: held under spinlock, UnsafeCell gives interior mutability.
        buf[..len].copy_from_slice(unsafe { &(&*self.hostname.get())[..len] });
        len
    }

    pub fn set_hostname(&self, src: &[u8]) -> bool {
        if src.len() >= HOSTNAME_MAX {
            return false;
        }
        let _g = self.lock.lock();
        // SAFETY: sous spinlock, UnsafeCell protège l'accès exclusif.
        unsafe {
            let h: &mut [u8; HOSTNAME_MAX] = &mut *self.hostname.get();
            h[..src.len()].copy_from_slice(src);
        }
        self.hostname_len.store(src.len() as u32, Ordering::Release);
        true
    }

    pub fn inc_ref(&self) {
        self.refcount.fetch_add(1, Ordering::Relaxed);
    }
    pub fn dec_ref(&self) -> u32 {
        self.refcount.fetch_sub(1, Ordering::AcqRel)
    }
}

unsafe impl Sync for UtsNamespace {}

/// Namespace UTS racine.
pub static ROOT_UTS_NS: UtsNamespace = UtsNamespace::new_root();
