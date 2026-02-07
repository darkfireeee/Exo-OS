//! Système de stockage pour les résultats de threads
//!
//! Fournit un mécanisme thread-safe pour stocker et récupérer les résultats
//! de threads terminés.

extern crate alloc;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::sync::Mutex;
use super::ThreadId;

/// Stockage global pour les résultats de threads
static THREAD_RESULTS: Mutex<Option<BTreeMap<ThreadId, Box<dyn Any + Send>>>> = Mutex::new(None);
static NEXT_SLOT_ID: AtomicU64 = AtomicU64::new(0);

/// Initialise le système de stockage
pub(crate) fn init_storage() {
    let mut storage = THREAD_RESULTS.lock().unwrap();
    if storage.is_none() {
        *storage = Some(BTreeMap::new());
    }
}

/// Stocke le résultat d'un thread
pub(crate) fn store_result<T: Send + 'static>(thread_id: ThreadId, value: T) {
    init_storage();
    let mut storage = THREAD_RESULTS.lock().unwrap();
    if let Some(ref mut map) = *storage {
        map.insert(thread_id, Box::new(value));
    }
}

/// Récupère le résultat d'un thread
pub(crate) fn take_result<T: Send + 'static>(thread_id: ThreadId) -> Option<T> {
    init_storage();
    let mut storage = THREAD_RESULTS.lock().unwrap();
    if let Some(ref mut map) = *storage {
        if let Some(boxed_any) = map.remove(&thread_id) {
            // Tente de downcast vers T
            match boxed_any.downcast::<T>() {
                Ok(boxed_t) => return Some(*boxed_t),
                Err(_) => return None,
            }
        }
    }
    None
}

/// Nettoie le résultat d'un thread (sans le retourner)
pub(crate) fn cleanup_result(thread_id: ThreadId) {
    init_storage();
    let mut storage = THREAD_RESULTS.lock().unwrap();
    if let Some(ref mut map) = *storage {
        map.remove(&thread_id);
    }
}

/// Alloue un slot pour un nouveau thread
pub(crate) fn allocate_slot() -> ThreadId {
    NEXT_SLOT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Taille actuelle du stockage (pour diagnostics)
pub(crate) fn storage_size() -> usize {
    let storage = THREAD_RESULTS.lock().unwrap();
    storage.as_ref().map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_basic() {
        let tid = allocate_slot();
        store_result(tid, 42u32);
        assert_eq!(take_result::<u32>(tid), Some(42));
        assert_eq!(take_result::<u32>(tid), None); // Déjà consommé
    }

    #[test]
    fn test_storage_multiple() {
        let tid1 = allocate_slot();
        let tid2 = allocate_slot();

        store_result(tid1, 100i64);
        store_result(tid2, 200i64);

        assert_eq!(take_result::<i64>(tid2), Some(200));
        assert_eq!(take_result::<i64>(tid1), Some(100));
    }

    #[test]
    fn test_storage_wrong_type() {
        let tid = allocate_slot();
        store_result(tid, 42u32);
        assert_eq!(take_result::<i64>(tid), None); // Mauvais type
    }

    #[test]
    fn test_cleanup() {
        let tid = allocate_slot();
        store_result(tid, 999u64);
        cleanup_result(tid);
        assert_eq!(take_result::<u64>(tid), None);
    }
}
