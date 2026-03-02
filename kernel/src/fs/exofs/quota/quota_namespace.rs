//! QuotaNamespace — isolation de quotas par espace de noms ExoFS (no_std).

use alloc::collections::BTreeMap;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;
use super::quota_policy::{QuotaPolicy, QuotaLimits};

/// Identifiant de namespace de quota.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NamespaceId(pub u64);

/// Un namespace de quota regroupe une politique applicable à un sous-volume ou
/// un conteneur.
#[derive(Clone, Debug)]
pub struct QuotaNamespaceEntry {
    pub id:     NamespaceId,
    pub name:   [u8; 32],   // Nom UTF-8 paddé de zéros.
    pub policy: QuotaPolicy,
}

impl QuotaNamespaceEntry {
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(32);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }
}

/// Registre global des namespaces de quota.
pub static QUOTA_NAMESPACE: QuotaNamespace = QuotaNamespace::new_const();

pub struct QuotaNamespace {
    entries: SpinLock<BTreeMap<NamespaceId, QuotaNamespaceEntry>>,
}

impl QuotaNamespace {
    pub const fn new_const() -> Self {
        Self { entries: SpinLock::new(BTreeMap::new()) }
    }

    pub fn register(
        &self,
        id: NamespaceId,
        name: &[u8],
        policy: QuotaPolicy,
    ) -> Result<(), FsError> {
        let mut entries = self.entries.lock();
        if entries.contains_key(&id) {
            return Err(FsError::InvalidArgument);
        }
        let mut name_arr = [0u8; 32];
        let n = name.len().min(32);
        name_arr[..n].copy_from_slice(&name[..n]);

        entries.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        entries.insert(id, QuotaNamespaceEntry { id, name: name_arr, policy });
        Ok(())
    }

    pub fn remove(&self, id: &NamespaceId) -> bool {
        self.entries.lock().remove(id).is_some()
    }

    pub fn get_policy(&self, id: &NamespaceId) -> Option<QuotaPolicy> {
        self.entries.lock().get(id).map(|e| e.policy.clone())
    }

    pub fn set_limits(&self, id: &NamespaceId, limits: QuotaLimits) -> Result<(), FsError> {
        let mut entries = self.entries.lock();
        if let Some(e) = entries.get_mut(id) {
            e.policy.user_limits    = limits;
            e.policy.group_limits   = limits;
            e.policy.project_limits = limits;
            Ok(())
        } else {
            Err(FsError::NotFound)
        }
    }

    pub fn len(&self) -> usize { self.entries.lock().len() }
}
