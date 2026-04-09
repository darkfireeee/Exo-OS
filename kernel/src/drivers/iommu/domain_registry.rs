//! # drivers/iommu/domain_registry.rs
//!
//! Registre bidirectionnel PID <-> domaine IOMMU.
//! Implémentation fixe, sans allocation, conforme à CORR-16/CORR-23.

use core::sync::atomic::{AtomicU32, Ordering};

use spin::Mutex;

use crate::memory::dma::core::types::IommuDomainId;

const MAX_IOMMU_DOMAINS: usize = 256;
const MAX_DRIVER_DOMAINS: usize = MAX_IOMMU_DOMAINS - 1;

#[derive(Clone, Copy, Debug)]
struct DomainSlot {
    pid: u32,
    domain: IommuDomainId,
}

struct DomainRegistryInner {
    pid_to_domain: [Option<DomainSlot>; MAX_DRIVER_DOMAINS],
    domain_to_pid: [Option<u32>; MAX_IOMMU_DOMAINS],
}

impl DomainRegistryInner {
    const fn new() -> Self {
        Self {
            pid_to_domain: [None; MAX_DRIVER_DOMAINS],
            domain_to_pid: [None; MAX_IOMMU_DOMAINS],
        }
    }
}

pub struct IommuDomainRegistry {
    inner: Mutex<DomainRegistryInner>,
    next_domain_id: AtomicU32,
}

impl IommuDomainRegistry {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(DomainRegistryInner::new()),
            next_domain_id: AtomicU32::new(1),
        }
    }

    pub fn ensure_domain(&self, pid: u32) -> Result<IommuDomainId, ()> {
        if pid == 0 {
            return Ok(IommuDomainId(0));
        }

        let mut inner = self.inner.lock();

        if let Some(slot) = inner.pid_to_domain.iter().flatten().find(|slot| slot.pid == pid) {
            return Ok(slot.domain);
        }

        let domain_raw = self.next_domain_id.fetch_add(1, Ordering::Relaxed);
        if domain_raw == 0 || domain_raw as usize >= MAX_IOMMU_DOMAINS {
            return Err(());
        }

        let Some(slot) = inner.pid_to_domain.iter_mut().find(|slot| slot.is_none()) else {
            return Err(());
        };

        let domain = IommuDomainId(domain_raw);
        *slot = Some(DomainSlot { pid, domain });
        inner.domain_to_pid[domain_raw as usize] = Some(pid);
        Ok(domain)
    }

    pub fn domain_of_pid(&self, pid: u32) -> Result<IommuDomainId, ()> {
        if pid == 0 {
            return Ok(IommuDomainId(0));
        }

        let inner = self.inner.lock();
        inner.pid_to_domain
            .iter()
            .flatten()
            .find(|slot| slot.pid == pid)
            .map(|slot| slot.domain)
            .ok_or(())
    }

    pub fn pid_of_domain(&self, domain_id: IommuDomainId) -> Option<u32> {
        if domain_id.0 as usize >= MAX_IOMMU_DOMAINS {
            return None;
        }
        self.inner.lock().domain_to_pid[domain_id.0 as usize]
    }

    pub fn release_domain(&self, pid: u32) {
        if pid == 0 {
            return;
        }

        let mut inner = self.inner.lock();
        if let Some(slot) = inner
            .pid_to_domain
            .iter_mut()
            .find(|slot| slot.map(|entry| entry.pid == pid).unwrap_or(false))
            .and_then(Option::take)
        {
            if (slot.domain.0 as usize) < MAX_IOMMU_DOMAINS {
                inner.domain_to_pid[slot.domain.0 as usize] = None;
            }
        }
    }
}

pub static IOMMU_DOMAIN_REGISTRY: IommuDomainRegistry = IommuDomainRegistry::new();
