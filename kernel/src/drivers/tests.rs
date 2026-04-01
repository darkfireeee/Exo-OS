#[cfg(test)]
pub mod smp_tests {
    extern crate std;
    use std::thread;
    use std::sync::Arc;
    use std::time::Instant;
    use crate::drivers::iommu::fault_queue::{IommuFaultQueue, IommuFaultEvent};
    use crate::drivers::device_claims::{sys_pci_claim, DEVICE_CLAIMS, PciBdf, ClaimError};
    use crate::memory::core::types::PhysAddr;
    use core::sync::atomic::Ordering;

    #[test]
    fn test_01_iommu_queue_hft_smp_stress() {
        println!("\n[TEST 1] HFT SMP Multi-core Stress Test");
        let queue = Arc::new(IommuFaultQueue::new());
        queue.init();

        let producers = 16;
        let consumers = 16;
        let items = 10_000;
        let expected = producers * items;

        let total_popped = Arc::new(core::sync::atomic::AtomicUsize::new(0));
        let mut handles = std::vec::Vec::new();

        let start = Instant::now();

        // 16 consumers acharnés
        for _ in 0..consumers {
            let q = queue.clone();
            let cp = total_popped.clone();
            handles.push(thread::spawn(move || {
                let target = expected / consumers;
                let mut local = 0;
                while local < target {
                    if q.pop().is_some() {
                        local += 1;
                        cp.fetch_add(1, Ordering::Relaxed);
                    } else {
                        core::hint::spin_loop();
                    }
                }
            }));
        }

        // 16 producteurs acharnés
        for p in 0..producers {
            let q = queue.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..items {
                    let ev = IommuFaultEvent { device_id: p as u16, fault_type: 0, domain_id: 0, faulted_addr: 0 };
                    while !q.push(ev) {
                        core::hint::spin_loop();
                    }
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let dur = start.elapsed();
        println!("-> SUCCES : {} evt traites par 32 coeurs en {:?} (Zero Lock Mutex)", total_popped.load(Ordering::Relaxed), dur);
        assert_eq!(total_popped.load(Ordering::Relaxed), expected as usize);
    }

    #[test]
    fn test_02_toctou_pci_claim() {
        println!("\n[TEST 2] TOCTOU System PCI Claim Stress Test");
        // Simulation d'une attaque TOCTOU : 50 threads tentent d'enregistrer le BDF 0x01.00.0 simultanement
        let phys_base = PhysAddr::new(0xA000_0000);
        let size = 4096;
        let bdf = Some(PciBdf { bus: 1, dev: 0, func: 0 });

        let mut handles = std::vec::Vec::new();
        let start = Instant::now();

        for i in 0..50 {
            handles.push(thread::spawn(move || {
                // Seulement un thread passera le check initial TOCTOU. Le reste aura AlreadyClaimed.
                sys_pci_claim(phys_base, size, i, bdf, 0)
            }));
        }

        let mut successes = 0;
        let mut failures_already_claimed = 0;

        for h in handles {
            match h.join().unwrap() {
                Ok(_) => successes += 1,
                Err(ClaimError::AlreadyClaimed) => failures_already_claimed += 1,
                Err(e) => panic!("Erreur inattendue {:?}", e),
            }
        }

        println!("-> SUCCES TOCTOU : {:?} ecoule. Resultats : {} succes, {} bloqués.", 
                 start.elapsed(), successes, failures_already_claimed);

        assert_eq!(successes, 1);
        assert_eq!(failures_already_claimed, 49);
        
        // Cleanup test environment
        DEVICE_CLAIMS.write().clear();
    }
}
