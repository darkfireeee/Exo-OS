# ExoOS — Guide d'Implémentation GI-03
## Driver Framework : IRQ, DMA, PCI, IOMMU

**Prérequis** : GI-01 (types), GI-02 (boot minimal fonctionnel)  
**Produit** : `kernel/src/arch/x86_64/irq/`, `kernel/src/drivers/`

---

## 1. Ordre d'Implémentation

```
Étape 1 : irq/types.rs           ← IrqSourceKind, IrqAckResult, IrqHandler
Étape 2 : irq/routing.rs         ← IrqRoute struct + IRQ_TABLE static
Étape 3 : sys_irq_register()     ← Enregistrement handlers (avec purge PIDs morts)
Étape 4 : dispatch_irq()         ← Dispatch ISR (SANS Vec, SANS yield)
Étape 5 : ack_irq()              ← ACK handlers + watchdog logic
Étape 6 : irq/watchdog.rs        ← Surveillance masked_since
Étape 7 : iommu/fault_queue.rs   ← IommuFaultQueue CAS-based
Étape 8 : iommu/fault_handler.rs ← ISR + worker tick
Étape 9 : dma.rs                 ← sys_dma_map (COW avant perms)
Étape 10: device_claims.rs       ← sys_pci_claim (TOCTOU protégé)
Étape 11: pci_topology.rs        ← PciTopology 1024 + irq_save
Étape 12: process/lifecycle.rs   ← do_exit() ordre strict
```

---

## 2. dispatch_irq — Règles ISR Absolues

```rust
// kernel/src/arch/x86_64/irq/routing.rs
//
// dispatch_irq s'exécute en ISR (Interrupt Service Routine) context.
// RÈGLES ISR ABSOLUES :
//
// ✅ AUTORISÉ en ISR :
//   - AtomicXxx operations
//   - lapic_send_eoi() / ioapic_mask() / ioapic_unmask()
//   - log::error!/warn! (SI implémenté avec ring buffer statique sans alloc)
//   - ipc::send_irq_notification() (enqueue non-bloquant dans SpscRing)
//   - core::hint::spin_loop() (avec limite SPIN_THRESHOLD)
//
// ❌ INTERDIT en ISR :
//   - Vec::new() / vec![] / .collect() → ALLOC HEAP
//   - spin::Mutex::lock() → PEUT BLOQUER
//   - scheduler::yield_current_thread() → CORRUPTION PILE (FIX-109)
//   - timer::sleep_ms() → BLOQUANT
//   - Toute opération pouvant bloquer indéfiniment
//
// ERREUR SILENCIEUSE v2-03 :
//   MAX_HANDLERS_PER_IRQ = 8 crée une limite fixe.
//   Si un driver crash sans cleanup, ses handlers restent occupés.
//   → sys_irq_register doit purger les handlers orphelins (CORR-51)

const MAX_HANDLERS_PER_IRQ: usize = 8;
const MAX_PENDING_ACKS:     u32   = 4096;  // Seuil de storm (FIX-87/88)
const SPIN_THRESHOLD:       u32   = 8;     // Max spins en ISR avant drop

pub fn dispatch_irq(irq: u8) {
    let table = IRQ_TABLE.read();

    let Some(route) = &table[irq as usize] else {
        drop(table);
        lapic_send_eoi(); // EOI même sans handler (évite blocage LAPIC)
        return;
    };

    // ─── Vérification Blacklist EN PREMIER ────────────────────────────
    // Avant toute modification de state → évite état incohérent
    if route.overflow_count.load(Ordering::Relaxed) >= MAX_OVERFLOWS {
        lapic_send_eoi(); // FIX-108 : EOI TOUJOURS (même blacklisté)
        return;
    }

    // ─── Protocole EOI selon source kind ──────────────────────────────
    match route.source_kind {
        IrqSourceKind::IoApicLevel => {
            ioapic_mask(irq); // Masquer AVANT traitement (évite re-trigger)
            // EOI LAPIC DIFFÉRÉ → envoyé dans ack_irq(remaining==0)
            let now = current_time_ms();
            let _ = route.masked_since.compare_exchange(
                0, now, Ordering::Release, Ordering::Relaxed
                //          ↑ Release : publie `now` pour watchdog (Acquire)
            );
            route.masked.store(true, Ordering::Release);
        }
        IrqSourceKind::IoApicEdge | IrqSourceKind::Msi | IrqSourceKind::MsiX => {
            lapic_send_eoi(); // EOI IMMÉDIAT (pas de re-trigger possible)
            let now = current_time_ms();
            let _ = route.masked_since.compare_exchange(
                0, now, Ordering::Release, Ordering::Relaxed
            );
            route.masked.store(false, Ordering::Release);
        }
    }

    // ─── Collecter endpoints SANS ALLOC HEAP (CORR-04) ────────────────
    let mut eps:  [Option<IpcEndpoint>; MAX_HANDLERS_PER_IRQ] = [None; MAX_HANDLERS_PER_IRQ];
    let mut n_eps = 0usize;

    for h in route.handlers.iter() {
        if n_eps >= MAX_HANDLERS_PER_IRQ { break; } // Truncation (CORR-37 prévient)
        eps[n_eps] = Some(h.endpoint);
        n_eps += 1;
    }
    let n = n_eps as u32;

    // ─── Incrémenter génération de vague ──────────────────────────────
    let wg = route.dispatch_generation.fetch_add(1, Ordering::AcqRel) + 1;

    // ─── Mise à jour pending_acks ──────────────────────────────────────
    match route.source_kind {
        IrqSourceKind::IoApicLevel => {
            route.pending_acks.store(n, Ordering::Release);
        }
        _ => {
            // CAS loop avec limite pour ISR (CORR-19 : spin_count reset)
            let mut current    = route.pending_acks.load(Ordering::Relaxed);
            let mut spin_count = 0u32;
            loop {
                if current > MAX_PENDING_ACKS {
                    // IRQ storm détectée
                    let now = current_time_ms();
                    let _ = route.masked_since.compare_exchange(
                        0, now, Ordering::Release, Ordering::Relaxed
                    );
                    match route.pending_acks.compare_exchange(
                        current, n, Ordering::Release, Ordering::Relaxed
                    ) {
                        Ok(_) => {
                            let ov = route.overflow_count.fetch_add(1, Ordering::Relaxed) + 1;
                            if ov >= MAX_OVERFLOWS {
                                ioapic_mask(irq);
                                route.pending_acks.store(0, Ordering::Release);
                                device_server_ipc::notify_irq_blacklisted(irq);
                                return;
                            }
                            device_server_ipc::notify_driver_stall(irq);
                            break;
                        }
                        Err(actual) => {
                            current = actual;
                            spin_count += 1; // CORR-19 : incrémenté ici aussi
                            if spin_count >= SPIN_THRESHOLD {
                                log::warn!("IRQ {} CAS contention extrême → drop", irq);
                                return; // JAMAIS yield en ISR
                            }
                            core::hint::spin_loop();
                        }
                    }
                } else {
                    match route.pending_acks.compare_exchange(
                        current, current + n, Ordering::AcqRel, Ordering::Relaxed
                    ) {
                        Ok(prev) => {
                            if prev == 0 {
                                route.overflow_count.store(0, Ordering::Relaxed);
                            }
                            break;
                        }
                        Err(actual) => {
                            current = actual;
                            spin_count += 1; // CORR-19 : incrémenté ici aussi
                            if spin_count >= SPIN_THRESHOLD {
                                log::warn!("IRQ {} CAS normal drop", irq);
                                return;
                            }
                            core::hint::spin_loop();
                        }
                    }
                }
            }
        }
    }

    // ─── Dispatch IPC vers handlers ────────────────────────────────────
    for i in 0..n_eps {
        if let Some(ep) = eps[i] {
            ipc::send_irq_notification(&ep, irq, wg);
        }
    }
}
```

---

## 3. sys_irq_register — Purge PIDs Morts (CORR-51)

```rust
// sys_irq_register — version avec purge handlers orphelins

pub fn sys_irq_register(
    irq:         u8,
    endpoint:    IpcEndpoint,
    source_kind: IrqSourceKind,
    bdf:         Option<PciBdf>,
) -> Result<u64, IrqError> {
    // Vérifier vecteur réservé (CORR-44)
    if irq >= VECTOR_RESERVED_START {
        return Err(IrqError::VectorReserved);
    }

    let _irq_guard = arch::irq_save();
    let mut table  = IRQ_TABLE.write();

    let route = table[irq as usize].get_or_insert_with(|| IrqRoute::new(irq, source_kind));

    // CORR-51 : Purger les handlers de PIDs morts AVANT test de limite
    // Cas : driver crash brutal sans cleanup via do_exit()
    // process::is_alive() retourne false si PID terminé/zombie/inexistant
    route.handlers.retain(|h| {
        let alive = process::is_alive(h.owner_pid);
        if !alive {
            log::debug!("IRQ {}: purge handler orphelin PID {}", irq, h.owner_pid);
        }
        alive
    });

    // Vérification kind (FIX-67)
    if route.handlers.len() > 0 && route.source_kind != source_kind {
        return Err(IrqError::KindMismatch {
            existing: route.source_kind, requested: source_kind
        });
    }

    // Test de limite APRÈS purge (CORR-37)
    if route.handlers.len() >= MAX_HANDLERS_PER_IRQ {
        return Err(IrqError::HandlerLimitReached);
    }

    // Reset handlers obsolètes du même PID (FIX-99, FIX-112)
    let calling_pid = current_process::pid();
    route.handlers.retain(|h| h.owner_pid != calling_pid);
    route.overflow_count.store(0, Ordering::Relaxed);
    if route.pending_acks.load(Ordering::Acquire) == 0 {
        route.handled_count.store(0, Ordering::Relaxed);
    }

    let generation = GLOBAL_GEN.fetch_add(1, Ordering::Relaxed);
    let reg_id     = new_reg_id();

    route.handlers.push(IrqHandler {
        reg_id, generation, owner_pid: calling_pid, endpoint
    }).map_err(|_| IrqError::HandlerLimitReached)?;

    Ok(reg_id)
}
```

---

## 4. IommuFaultQueue — Implementation CAS-based

```rust
// kernel/src/drivers/iommu/fault_queue.rs
//
// ERREURS SILENCIEUSES FRÉQUENTES :
//
// ❌ init() Ordering::Relaxed (était dans v9)
//    → Sur ARM/RISC-V, les stores seq[0..63] peuvent être vus APRÈS
//      initialized.store(true) par un autre CPU
//    → push() commence avant que les slots soient prêts → drops silencieux
//    ✅ Tous les stores dans init() doivent être Release (FIX-100)
//
// ❌ compare_exchange_weak (était dans v9)
//    → Sur ARM/RISC-V, false spurious failures → drops incorrects
//    ✅ compare_exchange (strong) uniquement (FIX-104)
//
// ❌ APPELER push() AVANT init()
//    → slot[0..62] ont seq=0 ≠ leur index attendu → tous droppés
//    → Mode debug : debug_assert!(initialized) attrape ça
//    → Mode release : drop silencieux des premières fautes IOMMU

pub fn init(&self) {
    // Initialiser seq[i] = i pour chaque slot
    for (i, slot) in self.slots.iter().enumerate() {
        // Release : garantit visibilité sur tous CPUs
        slot.seq.store(i, Ordering::Release);
    }
    self.initialized.store(true, Ordering::Release);
}

pub fn push(&self, event: IommuFaultEvent) -> bool {
    // Précondition : init() doit avoir été appelé
    debug_assert!(
        self.initialized.load(Ordering::Acquire),
        "IommuFaultQueue.push() avant init() — activer IRQs IOMMU APRÈS init()"
    );

    let pos = self.head.load(Ordering::Relaxed);
    let idx = pos % IOMMU_QUEUE_CAPACITY;
    let slot = &self.slots[idx];

    // Slot libre pour ce pos ?
    if slot.seq.load(Ordering::Acquire) != pos {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        return false;
    }

    // CAS strong (pas weak) — portabilité ARM/RISC-V (FIX-104)
    match self.head.compare_exchange(
        pos, pos + 1, Ordering::AcqRel, Ordering::Relaxed
    ) {
        Ok(_) => {
            unsafe { *slot.event.get() = event; }
            slot.seq.store(pos + 1, Ordering::Release);
            true
        }
        Err(_) => {
            // Contention : autre ISR a pris ce slot
            self.dropped.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}
```

---

## 5. sys_dma_map — Ordre COW Avant Perms

```rust
// kernel/src/drivers/dma.rs
//
// ORDRE IMPÉRATIF : COW AVANT query_perms (FIX-68)
//
// ❌ ERREUR SILENCIEUSE : query_perms AVANT resolve_cow
//    Scénario : page COW (ex: après fork()) = partagée, marquée PROT_READ
//    query_perms voit PROT_READ → retourne PermissionDenied
//    Pourtant, le DMA write est légitime (la page deviendra privée après COW)
//    → Driver DMA légitime ne peut pas mapper ses buffers malloc()
//
// L'ordre correct :
//   1. resolve_cow_or_fault() → crée une copie privée si nécessaire
//   2. query_perms()          → maintenant la page est privée, perms correctes
//   3. pin_user_page()        → épingler la page (empêche le swap)
//   4. iommu::map_page()      → créer le mapping IOVA

pub fn sys_dma_map(vaddr: usize, size: usize, dir: DmaDirection) -> Result<IoVirtAddr, DmaError> {
    let pid = current_process::pid();
    let page_count = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    let mut pinned: heapless::Vec<PinnedPage, MAX_DMA_PAGES> = heapless::Vec::new();

    for i in 0..page_count {
        let vpage = vaddr + i * PAGE_SIZE;

        // Étape 1 : COW AVANT query_perms (FIX-68 obligatoire)
        if matches!(dir, DmaDirection::FromDevice | DmaDirection::Bidirectional) {
            page_tables::resolve_cow_or_fault(pid, vpage, PageProtection::WRITE)
                .map_err(|e| {
                    // Rollback des pages déjà épinglées
                    for p in &pinned { p.unpin(); }
                    match e {
                        CowError::OutOfMemory => DmaError::OutOfMemory,
                        _                     => DmaError::InvalidVaddr,
                    }
                })?;
        }

        // Étape 2 : Vérifier les permissions APRÈS COW
        let perms = page_tables::query_perms_single(pid, vpage)
            .ok_or_else(|| { for p in &pinned { p.unpin(); } DmaError::InvalidVaddr })?;

        if matches!(dir, DmaDirection::FromDevice | DmaDirection::Bidirectional)
            && !perms.is_writable()
        {
            for p in &pinned { p.unpin(); }
            return Err(DmaError::PermissionDenied);
        }

        // Étape 3 : Épingler la page (empêche swap pendant DMA)
        let p = page_tables::pin_user_page(pid, vpage)
            .ok_or_else(|| { for p in &pinned { p.unpin(); } DmaError::InvalidVaddr })?;
        pinned.push(p).map_err(|_| DmaError::IovaSpaceExhausted)?;
    }

    // Étape 4 : Allouer une plage IOVA dans l'espace IOMMU du driver
    let domain_id = IOMMU_DOMAIN_REGISTRY.domain_of_pid(pid)?;
    let iova_base = iommu::alloc_iova_range(domain_id, page_count)?;

    // Étape 5 : Créer les mappings IOMMU (avec rollback en cas d'erreur)
    for (i, p) in pinned.iter().enumerate() {
        let iova = IoVirtAddr(iova_base.0 + (i * PAGE_SIZE) as u64);
        if let Err(_) = iommu::map_page(domain_id, iova, p.phys_addr(), dir) {
            for j in 0..i {
                iommu::unmap_page(domain_id, IoVirtAddr(iova_base.0 + (j * PAGE_SIZE) as u64));
            }
            for p in &pinned { p.unpin(); }
            return Err(DmaError::IommuError);
        }
    }

    dma_map_table::register(pid, domain_id, iova_base, pinned, size);
    Ok(iova_base)
}
```

---

## 6. sys_pci_claim — TOCTOU Protection (CORR-32)

```rust
// kernel/src/drivers/device_claims.rs
//
// ERREUR SILENCIEUSE (TOCTOU) :
//   Vérifier MMIO_WHITELIST.contains() PUIS prendre DEVICE_CLAIMS.write()
//   → Un autre thread peut claimer la même région entre les deux
//   → Deux drivers obtiennent le même claim
//   → Corruption DMA / accès MMIO incorrect
//
// ✅ CORRECT : Prendre le lock EN PREMIER, vérifier SOUS le lock

pub fn sys_pci_claim(
    phys_base:   PhysAddr,
    size:        usize,
    driver_pid:  u32,
    bdf:         Option<PciBdf>,
    calling_pid: u32,
) -> Result<(), ClaimError> {
    // Vérification capability AVANT lock (lecture seule, pas de TOCTOU ici)
    if !process::has_capability(calling_pid, Capability::SysDeviceAdmin) {
        return Err(ClaimError::PermissionDenied);
    }

    // CORR-32 : Lock AVANT toute vérification de région
    let _irq = arch::irq_save(); // Éviter deadlock IRQ
    let mut claims = DEVICE_CLAIMS.write();

    // Toutes les vérifications SOUS le lock
    if !MMIO_WHITELIST.contains(phys_base, size) {
        return Err(ClaimError::NotInHardwareRegion);
    }
    if memory_map::is_ram_region(phys_base, size) {
        return Err(ClaimError::PhysIsRam);
    }
    if claims.iter().any(|c| c.overlaps(phys_base, size)) {
        return Err(ClaimError::AlreadyClaimed);
    }
    // CORR-32 : Vérifier unicité BDF
    if let Some(b) = bdf {
        if claims.iter().any(|c| c.bdf == Some(b)) {
            return Err(ClaimError::AlreadyClaimed);
        }
    }

    let gen = process::get_generation(driver_pid);
    claims.push(DeviceClaim { phys_base, size, owner_pid: driver_pid, generation: gen, bdf })
        .map_err(|_| ClaimError::TableFull)?;

    Ok(())
}
```

---

## 7. do_exit() — Ordre Strict Impératif

```rust
// kernel/src/process/lifecycle.rs
//
// ORDRE IMPÉRATIF : chaque étape dépend des précédentes.
// Violation de l'ordre = corruption DMA / accès MMIO après libération.
//
// ❌ ERREUR : Révoquer IRQ AVANT DMA
//    → IRQ handler peut encore recevoir des notifications DMA
//    → Le driver est mort mais reçoit encore des IRQs
//    → Comportement indéfini du handler mort
//
// ❌ ERREUR : Révoquer MMIO AVANT attendre quiescence PCIe
//    → Device peut encore faire des DMA write vers des adresses libérées
//    → Corruption mémoire silencieuse (DMA after-free)
//
// ❌ ERREUR : Oublier iommu_domain_registry::release_domain()
//    → Fuite de domaines IOMMU → épuisement après N restarts de drivers

pub fn do_exit(pid: u32) {
    // ─── 1. Désactiver Bus Mastering ─────────────────────────────────
    // PREMIER : empêche de nouvelles transactions DMA
    pci::disable_bus_master_for_pid(pid);

    // ─── 2. Attendre quiescence PCIe ──────────────────────────────────
    let needs_reset = match pci::wait_bus_master_quiesced(pid, 100) {
        Ok(())                               => false,
        Err(PciError::BusMasterQuiesceTimeout) => true,
        Err(_)                               => false,
    };

    // ─── 3. Secondary Bus Reset si nécessaire ─────────────────────────
    if needs_reset {
        if let Some(bdf) = bdf_of_pid(pid) {
            if let Ok(()) = pci::secondary_bus_reset_bdf(bdf) {
                match wait_link_retraining(bdf, 200) {
                    Ok(()) => {}
                    Err(PciError::LinkTrainingTimeout) => {
                        if let Ok(domain_id) = IOMMU_DOMAIN_REGISTRY.domain_of_pid(pid) {
                            iommu::force_disable_domain(domain_id);
                        }
                    }
                    Err(e) => log::error!("PID {}: link retraining err {:?}", pid, e),
                }
            }
        } else {
            timer::sleep_ms(250); // Quarantaine aveugle si BDF inconnu
        }
    }

    // ─── 4. Révoquer mappings DMA temporaires ─────────────────────────
    dma_map_table::revoke_all_for_pid(pid);

    // ─── 5. Révoquer buffers DMA alloués (SYS_DMA_ALLOC) ─────────────
    dma::revoke_all_alloc_for_pid(pid);

    // ─── 6. Révoquer mappings MMIO ───────────────────────────────────
    mmio_cap::revoke_all_mmio(pid);

    // ─── 7. Désenregistrer handlers IRQ ──────────────────────────────
    irq::revoke_all_irq(pid);

    // ─── 8. Libérer claims PCI ────────────────────────────────────────
    device_claims::revoke_claims_for_pid(pid);

    // ─── 9. Libérer domaine IOMMU ────────────────────────────────────
    // DERNIER : après que tous les mappings sont révoqués
    IOMMU_DOMAIN_REGISTRY.release_domain(pid);
}
```

---

## 8. PciTopology — irq_save Obligatoire

```rust
// kernel/src/drivers/pci_topology.rs
//
// ERREUR SILENCIEUSE (DRV-45) : write() sans irq_save()
//   Scénario :
//     CPU-A : register() prend write lock
//     IRQ arrive sur CPU-A
//     IRQ handler appelle parent_bridge() → read lock
//     Spinlock read attend le write lock (CPU-A le tient)
//     CPU-A attend que l'IRQ handler finisse
//     DEADLOCK de cœur CPU → le CPU n'avancera plus
//
//   CLI (irq_save) désactive les IRQs normales → empêche ce scénario.
//   LIMITATION : NMI (Non-Maskable Interrupt) n'est PAS masquable par CLI.
//   → Les handlers NMI en Phase 8 NE DOIVENT PAS appeler parent_bridge()
//   → Phase 9 : remplacer RwLock par SeqLock (NMI-safe)

impl PciTopology {
    pub fn register(&self, child: PciBdf, parent: PciBdf) -> Result<(), PciError> {
        // OBLIGATOIRE : irq_save() AVANT write() (FIX-92 v8)
        let _irq_guard = arch::irq_save();
        self.entries.write()
            .push((child, parent))
            .map_err(|_| PciError::TopologyTableFull)
    }

    /// Appelable depuis thread ET ISR (read lock seulement).
    /// NE PAS appeler depuis un handler NMI (Phase 8).
    pub fn parent_bridge(&self, child: PciBdf) -> Option<PciBdf> {
        self.entries.read()
            .iter()
            .find(|(c, _)| *c == child)
            .map(|(_, p)| *p)
    }
}
```

---

## 9. Erreurs Silencieuses Spécifiques aux Drivers

| Erreur | Symptôme | Détection |
|--------|----------|-----------|
| Vec en ISR | Crash OOM aléatoire | CORR-04 : CI check + static |
| yield en ISR | Corruption pile kernel | FIX-109 : CI lint |
| CAS weak ARM | Drops spurieux IOMMU | FIX-104 : CI cross-arch |
| COW après perms | DMA write impossible (PermissionDenied) | FIX-68 : test fork+DMA |
| TOCTOU sys_pci_claim | Double claim possible | CORR-32 : test concurrent |
| PciTopology sans irq_save | Deadlock CPU rare | DRV-45 : test sous charge |
| do_exit sans domain_registry | Fuite domaines IOMMU | CORR-16 : count domains |
| EOI manquant blacklist Level | Système gelé (bit ISR LAPIC) | FIX-108 : test storm |
| masked_since Relaxed | Watchdog faux positifs | CORR-08 : test ARM |
| spin_count global | Drop boucle CAS normal | CORR-19 : test contention |

---

## 10. Tests de Validation Phase 2

```bash
# Test IRQ basique (1 IRQ, 1 handler)
# Déclencher IRQ via QEMU APIC → vérifier dispatch + ACK

# Test IRQ storm (Edge/MSI)
# Déclencher > MAX_PENDING_ACKS IRQs sans ACK
# ATTENDU : blacklist après MAX_OVERFLOWS, EOI toujours envoyé

# Test DMA fork+write
# Process fork → enfant alloue buffer malloc() → sys_dma_map(DmaDirection::FromDevice)
# ATTENDU : succès (COW résolu avant perms)

# Test double claim PCI
# Deux threads : sys_pci_claim(même BDF)
# ATTENDU : second call retourne AlreadyClaimed

# Test do_exit ordre
# Driver crash → do_exit() → vérifier aucun accès MMIO post-exit
# Via /proc/debug/mmio_access ou hook IOMMU fault
```

---

*ExoOS — Guide d'Implémentation GI-03 : Drivers IRQ DMA PCI — Mars 2026*
