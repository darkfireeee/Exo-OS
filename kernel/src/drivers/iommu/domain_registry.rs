//! # drivers/iommu/domain_registry.rs
//!
//! Registre bidirectionnel PID <-> domaine IOMMU.
//! Implémentation fixe, sans allocation côté registre, conforme à CORR-16/CORR-23.

use spin::Mutex;

use crate::memory::dma::core::types::IommuDomainId;
use crate::memory::dma::iommu::{DomainType, IOMMU_DOMAINS};

const MAX_IOMMU_DOMAINS: usize = 256;
const MAX_DRIVER_DOMAINS: usize = MAX_IOMMU_DOMAINS - 1;
const DOMAIN_IOVA_BASE: u64 = 0x0001_0000_0000;
const DOMAIN_IOVA_SLICE_SIZE: u64 = 0x0000_0100_0000;

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
}

impl IommuDomainRegistry {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(DomainRegistryInner::new()),
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

        let Some((slot_idx, slot)) = inner
            .pid_to_domain
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| slot.is_none())
        else {
            return Err(());
        };
        let iova_base = DOMAIN_IOVA_BASE.saturating_add((slot_idx as u64) * DOMAIN_IOVA_SLICE_SIZE);
        let iova_limit = iova_base.saturating_add(DOMAIN_IOVA_SLICE_SIZE);

        let domain = IOMMU_DOMAINS
            .create_domain(
                DomainType::Translated,
                iova_base,
                iova_limit,
            )
            .map_err(|_| ())?;
        let _ = IOMMU_DOMAINS.with_domain_mut(domain, |dom| dom.activate());

        if domain.0 == 0 || domain.0 as usize >= MAX_IOMMU_DOMAINS {
            let _ = IOMMU_DOMAINS.with_domain_mut(domain, |dom| dom.deactivate());
            return Err(());
        }

        *slot = Some(DomainSlot { pid, domain });
        inner.domain_to_pid[domain.0 as usize] = Some(pid);
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
            let _ = IOMMU_DOMAINS.with_domain_mut(slot.domain, |dom| dom.deactivate());
            let _ = IOMMU_DOMAINS.destroy_domain(slot.domain);
        }
    }
}

pub static IOMMU_DOMAIN_REGISTRY: IommuDomainRegistry = IommuDomainRegistry::new();
