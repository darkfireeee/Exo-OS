# ExoOS Kernel Types — Référence canonique v10
**Source unique de vérité pour tous les types partagés**
> Ce fichier prévaut sur tout extrait de code du document principal en cas de conflit.
> Importé implicitement par tous les modules `kernel/` et `servers/`.
>
> **Changelog v8→v10** (corrections post-audit multi-modèles) :
>   - FIX-103 : `BOOT_TSC_KHZ` → `AtomicU64` (static immutable = compile error Rust — Z-AI COMPIL-01)
>   - FIX-104 : `IommuFaultQueue::push()` → `compare_exchange` strong (portabilité ARM/RISC-V — KIMI/MINIMAX)
>   - FIX-105 : `PciTopology` — note de limitation NMI + path SeqLock Phase 9 (Z-AI INCOH-02)
>   - FIX-106 : imports atomics explicites dans tous les snippets (Z-AI COMPIL-02)
>   - FIX-107 : `DeviceClaim` — ajout note `handled_count` reset trigger via framework

---

## 1 — Adresses mémoire

```rust
// kernel/src/memory/addr.rs

/// Adresse physique DRAM — visible CPU uniquement.
/// NE DOIT JAMAIS être programmée dans un registre de device.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

/// Adresse IO virtuelle — visible par le device via l'IOMMU.
/// Seule adresse autorisée dans les registres DMA du device.
/// Obtenue via SYS_DMA_ALLOC ou SYS_DMA_MAP uniquement.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(transparent)]
pub struct IoVirtAddr(pub u64);

/// Adresse virtuelle dans l'espace d'un processus Ring 1.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct VirtAddr(pub usize);
```

---

## 2 — Protection mémoire

```rust
// kernel/src/memory/protection.rs

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct PageProtection(pub u8);

impl PageProtection {
    pub const READ:  Self = Self(0x01);
    pub const WRITE: Self = Self(0x02);
    pub const EXEC:  Self = Self(0x04);

    pub fn is_writable(self)   -> bool { self.0 & 0x02 != 0 }
    pub fn is_readable(self)   -> bool { self.0 & 0x01 != 0 }
    pub fn is_executable(self) -> bool { self.0 & 0x04 != 0 }
}
```

---

## 3 — Types IRQ

```rust
// kernel/src/arch/x86_64/irq/types.rs

/// Source d'une interruption. Détermine le protocole EOI et le comportement
/// de pending_acks (store vs fetch_add).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IrqSourceKind {
    /// Level-triggered IOAPIC.
    /// dispatch: mask IOAPIC + EOI différé (après dernier ACK handler).
    /// pending_acks: store(N). ACK stale: ignoré.
    IoApicLevel,
    /// Edge-triggered IOAPIC.
    /// dispatch: EOI immédiat. pending_acks: fetch_add(N). ACK stale: DOIT décrémenter.
    IoApicEdge,
    /// MSI — comportement identique à IoApicEdge. Pas d'IOAPIC impliqué.
    Msi,
    /// MSI-X — comportement identique à IoApicEdge. Pas d'IOAPIC impliqué.
    MsiX,
}

impl IrqSourceKind {
    /// Vrai si pending_acks est cumulatif (fetch_add) = Edge/MSI/MSI-X.
    /// Faux si pending_acks est unique (store) = Level.
    pub fn is_cumulative(self) -> bool {
        matches!(self, Self::IoApicEdge | Self::Msi | Self::MsiX)
    }
    /// Vrai si l'IRQ utilise l'IOAPIC (masquage matériel possible).
    pub fn needs_ioapic_mask(self) -> bool {
        matches!(self, Self::IoApicLevel | Self::IoApicEdge)
    }
}

/// Résultat d'acquittement d'une IRQ.
/// Le driver DOIT appeler SYS_IRQ_ACK même si résultat = NotMine (DRV-08).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum IrqAckResult {
    Handled = 0,
    NotMine = 1,
}

/// Enregistrement d'un handler IRQ dans une IrqRoute.
#[derive(Clone)]
pub struct IrqHandler {
    pub reg_id:     u64,
    pub generation: u64,   // génération du handler (anti ghost-handler DRV-05)
    pub owner_pid:  u32,
    pub endpoint:   IpcEndpoint,
}

/// Erreurs retournées par SYS_IRQ_REGISTER et SYS_IRQ_ACK.
#[derive(Debug)]
pub enum IrqError {
    NotRegistered,
    NotOwner,
    /// Tentative d'enregistrement d'un type de trigger incompatible sur une route existante.
    KindMismatch { existing: IrqSourceKind, requested: IrqSourceKind },
}
```

---

## 4 — Types DMA / IOMMU

```rust
// kernel/src/drivers/dma_types.rs

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DmaDirection {
    ToDevice,
    FromDevice,
    Bidirectional,
}

/// Erreurs retournées par SYS_DMA_MAP et SYS_DMA_ALLOC.
/// Note : DmaError::Overflow est supprimé (concept IRQ, hors scope DMA — FIX-87 v9).
#[derive(Debug)]
pub enum DmaError {
    /// Adresse virtuelle non présente ou invalide dans l'espace du processus.
    InvalidVaddr,
    /// Mémoire physique épuisée lors de resolve_cow_or_fault().
    OutOfMemory,
    /// DMA write demandé sur une page explicitement PROT_READ.
    /// Les pages COW standard ne déclenchent PAS cette erreur (résolues avant vérification).
    PermissionDenied,
    /// Erreur interne IOMMU (mapping, domaine introuvable).
    IommuError,
    /// Espace d'adressage IOVA épuisé pour ce domaine IOMMU.
    IovaSpaceExhausted,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum IommuFaultReason {
    #[default] Unknown,
    WriteToReadOnlyRegion,
    ReadFromWriteOnlyRegion,
    InvalidIova,
    DomainDisabled,
}
```

---

## 5 — Types PCI

```rust
// kernel/src/drivers/pci_types.rs

/// Adresse Bus/Device/Function d'un périphérique PCI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciBdf {
    pub bus:      u8,
    pub device:   u8,
    pub function: u8,
}

#[derive(Debug)]
pub enum PciError {
    InvalidBarIndex(u8),
    BarIsIo,
    BarNotImplemented,
    /// cfg_read32 retourne 0xFFFF_FFFF : device absent ou bus en erreur.
    DeviceAbsent,
    IsHighPartOf64BitBar { low_bar_idx: u8 },
    BusMasterQuiesceTimeout,
    LinkTrainingTimeout,
    /// Bridge parent introuvable dans PCI_TOPOLOGY.
    BridgeNotInTopology,
    /// Table de topologie pleine (> 1024 entrées).
    TopologyTableFull,
    /// Appelant sans capability SysDeviceAdmin.
    PermissionDenied,
}

#[derive(Debug)]
pub enum ClaimError {
    PermissionDenied,
    PhysIsRam,
    NotInHardwareRegion,
    AlreadyClaimed,
}

/// Enregistrement d'un claim sur une ressource hardware par un driver.
/// FIX-93 v8 : champ `bdf` ajouté pour permettre la récupération du BDF
/// des polling drivers (sans IRQ) dans do_exit() via bdf_of_pid().
pub struct DeviceClaim {
    pub phys_base:  PhysAddr,
    pub size:       usize,
    pub owner_pid:  u32,
    pub generation: u64,
    /// BDF du périphérique PCI associé.
    /// None pour les ressources non-PCI (ACPI, timer, etc.).
    /// Pour Phase 8 : un driver = un device → first match dans bdf_of_pid().
    pub bdf: Option<PciBdf>,
}
```

---

## 6 — Source temporelle

```rust
// kernel/src/time.rs
//
// FIX-103 v10 : BOOT_TSC_KHZ doit être AtomicU64, PAS `static u64`.
//
// POURQUOI v8/v9 était incorrect (Z-AI COMPIL-01) :
//   `static BOOT_TSC_KHZ: u64 = 0;` crée une variable globale IMMUTABLE en Rust.
//   calibrate_tsc_khz() ne peut pas la modifier → erreur de compilation :
//   "cannot assign to immutable static item `BOOT_TSC_KHZ`".
//
// CORRECTION v10 : AtomicU64 est la solution idiomatique pour les globales
// modifiées une seule fois au boot (write-once, read-many).
// Ordering::Relaxed suffit car la calibration se produit avant enable_interrupts()
// (garantie d'ordre séquentiel par la barrière enable_interrupts).

use core::sync::atomic::{AtomicU64, Ordering};

/// Fréquence TSC calibrée une seule fois au boot via PIT.
/// Exemple : 3_000_000 pour un CPU à 3 GHz.
/// Panique au boot si TSC non invariant (CPUID.80000007H:EDX[8]) et HPET absent.
static BOOT_TSC_KHZ: AtomicU64 = AtomicU64::new(0);

/// Source temps monotone (HPET préféré, TSC calibré en fallback).
/// Garantie monotone et cohérente entre tous les CPUs.
/// Ne jamais appeler _rdtsc() directement — les cycles ≠ millisecondes.
pub fn current_time_ms() -> u64 {
    if hpet::is_available() {
        return hpet::read_ms();
    }
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    let khz = BOOT_TSC_KHZ.load(Ordering::Relaxed);
    debug_assert!(khz > 0, "current_time_ms() appelé avant calibrate_tsc_khz()");
    tsc / khz
}

/// Calibration PIT au boot — écrit BOOT_TSC_KHZ une seule fois.
/// DOIT être appelé avant enable_interrupts() et avant tout appel à current_time_ms().
pub fn calibrate_tsc_khz() {
    // ... logique de calibration PIT ...
    let measured_khz: u64 = /* calibration PIT 50ms */ 3_000_000; // exemple 3 GHz
    BOOT_TSC_KHZ.store(measured_khz, Ordering::Relaxed);
    // Ordering::Relaxed suffit : enable_interrupts() sera la barrière mémoire
    // qui garantit la visibilité sur tous les CPUs avant le premier appel ISR.
}
```

---

## 7 — Queue de fautes IOMMU (MPSC CAS-based, ABA-free, ISR-safe)

```rust
// kernel/src/drivers/iommu/fault_queue.rs
//
// FIX-91 v8 : Remplacement du design fetch_add (v9) par CAS-based head acquisition.
// FIX-104 v10 : compare_exchange_weak → compare_exchange (strong) pour portabilité.
// FIX-106 v10 : imports explicites.
//
// POURQUOI le design fetch_add v9 était incorrect (Kimi CORR-1) :
//   push() fait head.fetch_add(1) pour obtenir pos, puis vérifie slot[pos%CAP].seq == pos.
//   Si la vérification échoue (queue pleine / contention), le head a déjà avancé
//   mais aucune donnée n'est écrite dans le slot. Le consumer attend seq == pos+1
//   mais ne l'obtient jamais (pos est "orphelin"). Après CAPACITY autres événements,
//   le même index revient avec une seq attendue différente → drop systématique.
//   En pratique : après 64 drops simultanés, la queue devient définitivement inutilisable.
//
// POURQUOI compare_exchange_weak est insuffisant (FIX-104) :
//   Sur x86_64, weak et strong compilent en LOCK CMPXCHG (pas de spurious failure).
//   Mais sur ARM (Aarch64) et RISC-V, weak peut échouer spuriously sans contention réelle,
//   incrémentant `dropped` sans raison → perte d'événements IOMMU légitimes.
//   compare_exchange (strong) évite cela sur toutes architectures.
//   Note Phase 8 : ExoOS cible x86_64 uniquement, mais la spec doit être portative.
//
// CONCEPTION v10 — CAS-based head :
//   push() utilise compare_exchange (strong) sur head. Si le slot est occupé ou si le
//   CAS échoue (contention réelle, jamais spurious), l'événement est dropped.
//   Head N'EST PAS avancé si on ne peut pas écrire → pas de slot orphelin possible.
//
//   PUSH (ISR-safe, non-bloquant) :
//     1. pos = head.load(Relaxed)
//     2. idx = pos % CAPACITY
//     3. Vérifier slot[idx].seq == pos          ← slot libre pour ce pos ?
//     4. Si non → dropped++ ; return false      ← queue pleine ou contention
//     5. CAS head : (pos → pos+1)               ← claim atomique du slot (STRONG)
//     6. Si CAS échoue → dropped++ ; return false (autre producer plus rapide)
//     7. Écrire event dans slot[idx]
//     8. slot[idx].seq.store(pos + 1, Release)  ← signaler au consumer
//
//   POP (worker thread, consumer unique) :
//     1. pos = tail.load(Relaxed)
//     2. idx = pos % CAPACITY
//     3. Si slot[idx].seq != pos + 1 → queue vide (return None)
//     4. Lire slot[idx].event
//     5. slot[idx].seq.store(pos + CAPACITY, Release)  ← libérer le slot
//     6. tail.store(pos + 1, Relaxed)
//
// PROPRIÉTÉS v10 :
//   • ABA-free : head n'avance que si le slot est disponible et le CAS réussit.
//   • Torn-read free : consumer lit seulement si seq == pos+1 (Release/Acquire).
//   • ISR-safe : push() ne bloque jamais ; retourne false si contention.
//   • No orphaned slots : head n'avance que pour des pushes réussis.
//   • No spurious drops : compare_exchange (strong), pas weak.
//   • Dropped events : correctement comptés, loggés par le worker.
//
// CONTRAINTE IMPORTANTE :
//   init() DOIT être appelé au boot avant toute activation des interruptions IOMMU.
//   Si push() est appelé avant init(), seul slot[0] fonctionnera correctement
//   (les autres slots ont seq=0 ≠ leur index attendu → drop).
//   Un debug_assert vérifie cette précondition en mode debug.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use core::cell::UnsafeCell;

const IOMMU_QUEUE_CAPACITY: usize = 64;

#[repr(align(64))] // alignement cache-line : évite false sharing entre slots
struct IommuFaultSlot {
    seq:   AtomicUsize,
    event: UnsafeCell<IommuFaultEvent>,
}

unsafe impl Sync for IommuFaultSlot {}

pub struct IommuFaultQueue {
    slots:       [IommuFaultSlot; IOMMU_QUEUE_CAPACITY],
    head:        AtomicUsize,   // prochaine position de production
    tail:        AtomicUsize,   // prochaine position de consommation
    dropped:     AtomicU64,     // événements perdus (queue pleine ou contention)
    initialized: AtomicBool,   // guard de précondition init()
}

impl IommuFaultQueue {
    pub const fn new() -> Self {
        // Tous les slots initialisés à seq=0 en const context.
        // init() corrige seq[i] = i pour tous i > 0.
        const INIT_SLOT: IommuFaultSlot = IommuFaultSlot {
            seq:   AtomicUsize::new(0),
            event: UnsafeCell::new(IommuFaultEvent::new_empty()),
        };
        IommuFaultQueue {
            slots:       [INIT_SLOT; IOMMU_QUEUE_CAPACITY],
            head:        AtomicUsize::new(0),
            tail:        AtomicUsize::new(0),
            dropped:     AtomicU64::new(0),
            initialized: AtomicBool::new(false),
        }
    }

    /// Appelé une seule fois au boot, AVANT activation des IRQs IOMMU.
    /// FIX-100 v8 + FIX-106 v10 : Ordering::Release pour garantir la visibilité
    /// sur tous les CPUs (par opposition à Ordering::Relaxed en v9 qui ne garantit
    /// pas la visibilité inter-CPU au moment où les ISRs démarrent).
    pub fn init(&self) {
        for (i, slot) in self.slots.iter().enumerate() {
            slot.seq.store(i, Ordering::Release); // seq[0]=0, seq[1]=1, ..., seq[63]=63
        }
        self.initialized.store(true, Ordering::Release);
    }

    /// Push ISR-safe. Ne bloque jamais. Retourne false si queue pleine ou contention.
    /// FIX-91 v8 : CAS-based — head n'avance QUE si l'écriture réussit (pas d'orphaned slots).
    /// FIX-104 v10 : compare_exchange (strong) — pas de spurious failure sur ARM/RISC-V.
    pub fn push(&self, event: IommuFaultEvent) -> bool {
        // Précondition en mode debug
        debug_assert!(
            self.initialized.load(Ordering::Acquire),
            "IOMMU_FAULT_QUEUE.push() appelé avant init() — activer les IRQs IOMMU seulement après init()"
        );

        let pos = self.head.load(Ordering::Relaxed);
        let idx = pos % IOMMU_QUEUE_CAPACITY;
        let slot = &self.slots[idx];

        // Vérifier que le slot est libre pour CE pos (seq == pos)
        if slot.seq.load(Ordering::Acquire) != pos {
            // Slot occupé par un événement non encore consommé → queue pleine
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        // Tenter de réserver le slot de façon atomique (STRONG — FIX-104)
        // FIX-91 : Si CAS échoue, un autre producteur a pris ce slot → drop propre,
        // HEAD N'EST PAS AVANCÉ → pas de slot orphelin.
        // FIX-104 : compare_exchange (strong) = pas de spurious failure sur ARM/RISC-V.
        match self.head.compare_exchange(
            pos, pos + 1, Ordering::AcqRel, Ordering::Relaxed,
        ) {
            Ok(_) => {
                // Slot exclusif pour ce producteur → écrire et signaler
                unsafe { *slot.event.get() = event; }
                slot.seq.store(pos + 1, Ordering::Release);
                true
            }
            Err(_) => {
                // CAS échoué : contention réelle (autre ISR simultané a pris ce slot)
                // Head non avancé → aucun orphaned slot possible
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Pop — consumer unique (worker thread kernel uniquement).
    pub fn pop(&self) -> Option<IommuFaultEvent> {
        let pos = self.tail.load(Ordering::Relaxed);
        let idx = pos % IOMMU_QUEUE_CAPACITY;
        let slot = &self.slots[idx];

        if slot.seq.load(Ordering::Acquire) != pos + 1 {
            return None; // queue vide
        }

        let event = unsafe { (*slot.event.get()).clone() };
        slot.seq.store(pos + IOMMU_QUEUE_CAPACITY, Ordering::Release);
        self.tail.store(pos + 1, Ordering::Relaxed);
        Some(event)
    }

    /// Retourne et remet à zéro le compteur d'événements perdus.
    pub fn drain_dropped(&self) -> u64 {
        self.dropped.swap(0, Ordering::Relaxed)
    }
}

#[derive(Clone, Debug)]
pub struct IommuFaultEvent {
    pub domain_id: u16,
    pub iova:      IoVirtAddr,
    pub reason:    IommuFaultReason,
    pub timestamp: u64,
}

impl IommuFaultEvent {
    pub const fn new_empty() -> Self {
        IommuFaultEvent {
            domain_id: 0,
            iova:      IoVirtAddr(0),
            reason:    IommuFaultReason::Unknown,
            timestamp: 0,
        }
    }
}

/// Instance globale unique — init() DOIT être appelé au boot.
pub static IOMMU_FAULT_QUEUE: IommuFaultQueue = IommuFaultQueue::new();
```

---

## 8 — Notifications device_server IPC

```rust
// kernel/src/drivers/device_server_ipc.rs
//
// Toutes les notifications kernel → device_server Ring 1.
// Aucune de ces fonctions ne bloque. Le device_server traite asynchrone.
// Jamais appelées depuis un ISR directement : toujours depuis worker ou thread kernel.

pub mod device_server_ipc {
    /// Driver non réactif : n'a pas ACKé son IRQ dans hard_ms.
    pub fn notify_driver_stall(irq: u8) { /* IPC non-bloquant */ }

    /// IRQ ghost : tous les handlers ont retourné NotMine sur IRQ level.
    pub fn notify_unhandled_irq(irq: u8) { /* IPC non-bloquant */ }

    /// IRQ blacklistée définitivement après MAX_OVERFLOWS storms.
    /// Le device_server doit désactiver le device et alerter l'opérateur.
    /// Pour unblacklist : reboot ou SYS_IRQ_UNBLACKLIST (nécessite SysDeviceAdmin).
    pub fn notify_irq_blacklisted(irq: u8) { /* IPC non-bloquant */ }

    /// Faute IOMMU : DMA non autorisé détecté par VT-d.
    /// Le device_server gère le lifecycle (kill driver, restart, alert).
    /// FIX-78 v8 : uniquement depuis iommu_fault_worker_tick(), jamais depuis ISR.
    pub fn notify_iommu_fault_kill(pid: u32, iova: IoVirtAddr, reason: IommuFaultReason) {
        /* IPC non-bloquant */
    }

    /// Fuite de mapping IOMMU détectée dans Drop de TempDmaMapping.
    pub fn notify_iommu_leak(domain_id: u32) { /* IPC non-bloquant */ }
}
```

---

## 9 — Topologie PCI

```rust
// kernel/src/drivers/pci_topology.rs
//
// FIX-92/96 v8 : capacité 1024, irq_save avant write lock.
// FIX-105 v10 : documentation de la limitation NMI + path SeqLock Phase 9.
//
// LIMITATION NMI (Z-AI INCOH-02) :
//   `irq_save` (CLI = Clear Interrupt Flag sur x86) désactive les interruptions
//   matérielles NORMALES. Cependant, les NMI (Non-Maskable Interrupts) ne sont PAS
//   masquables par CLI. Un NMI peut donc survenir pendant que register() tient le
//   write lock.
//
//   SCÉNARIO DE DEADLOCK :
//     CPU-0 : register() → irq_save() → entries.write() [LOCKED]
//     NMI arrive sur CPU-0 (MCE, watchdog hardware, etc.)
//     Handler NMI appelle do_exit() → wait_link_retraining() → parent_bridge()
//     parent_bridge() tente entries.read() → spin-wait sur write lock
//     CPU-0 tient le write lock et attend que le NMI handler finisse
//     → DEADLOCK DE CŒUR CPU.
//
//   MITIGATION PHASE 8 :
//     Les handlers NMI d'ExoOS Phase 8 doivent respecter la règle :
//     "Ne jamais appeler parent_bridge() ni aucune fonction qui prend entries.read()
//      depuis un context NMI."
//     En pratique : un handler NMI de Phase 8 se contente de logger et halter.
//     Il ne fait PAS de cleanup PCI — le système est considéré irrécupérable sur MCE.
//
//   PATH PHASE 9+ : SEQLOCK
//     Pour une NMI-safety complète, remplacer spin::RwLock par un SeqLock :
//     - Les lectures (parent_bridge) sont "wait-free" et ne prennent jamais de lock
//     - Les écritures (register) prennent le seqlock write exclusif
//     - Un reader qui voit une écriture en cours relance la lecture (retry)
//     - AUCUN DEADLOCK POSSIBLE depuis un handler NMI
//     cf. ExoOS_SeqLock_Design.md (Phase 9)
//
//   POUR PHASE 8 :
//     irq_save protège contre tous les cas d'interruptions ordinaires (99.9% des cas).
//     La fenêtre NMI est documentée comme limitation connue. Acceptable pour Phase 8
//     car les NMI handlers n'accèdent pas à PCI_TOPOLOGY dans cette phase.

use core::sync::atomic::Ordering;

pub struct PciTopology {
    // FIX-92 v8 : 1024 au lieu de 256 ; heapless conservé (const-constructible)
    // Un serveur EPYC avec SR-IOV peut exposer >256 fonctions PCI.
    // heapless::Vec<1024> ≈ 6 KB en BSS — négligeable.
    entries: spin::RwLock<heapless::Vec<(PciBdf, PciBdf), 1024>>,
    //                    (child_bdf, parent_bridge_bdf)
}

impl PciTopology {
    pub const fn new() -> Self {
        PciTopology {
            entries: spin::RwLock::new(heapless::Vec::new()),
        }
    }

    /// Retourne le BDF du bridge parent. None si device directement sur Root Complex.
    /// Appelable depuis thread et ISR (read lock seulement).
    /// NOTE : NE PAS appeler depuis un handler NMI (see LIMITATION NMI ci-dessus).
    pub fn parent_bridge(&self, child: PciBdf) -> Option<PciBdf> {
        self.entries.read()
            .iter()
            .find(|(c, _)| *c == child)
            .map(|(_, p)| *p)
    }

    /// Appelé par sys_pci_set_topology (SYS_PCI_SET_TOPOLOGY = 546).
    /// FIX-92 v8 : irq_save OBLIGATOIRE avant write lock pour éviter le deadlock
    /// décrit dans le header (CPU tient write lock → IRQ → read lock → deadlock).
    /// NOTE : CLI ne masque pas NMI (limitation documentée, acceptable Phase 8).
    pub fn register(&self, child: PciBdf, parent: PciBdf) -> Result<(), PciError> {
        let _irq_guard = arch::irq_save(); // désactiver IRQs locales (CLI)
        self.entries.write()
            .push((child, parent))
            .map_err(|_| PciError::TopologyTableFull)
    }
}

pub static PCI_TOPOLOGY: PciTopology = PciTopology::new();

// Constantes PCIe pour wait_link_retraining
pub const PCI_CAP_ID_EXP:       u8  = 0x10; // PCI Express Capability ID
pub const PCI_EXP_LNKSTA:       u16 = 0x12; // Link Status register (offset depuis base cap)
pub const PCI_EXP_LNKSTA_DLLLA: u16 = 1 << 13; // Data Link Layer Link Active

/// FIX-94 v8 : Lecture 16-bit sur l'espace config PCI (ECAM/CF8).
/// Implémentée via read32 + shift pour ne pas avoir à définir un nouveau syscall.
/// L'accès 16-bit aligné est valide sur PCIe ECAM selon la spec.
/// offset doit être aligné sur 2 octets (vérifié en debug).
pub fn pci_cfg_read16(bdf: PciBdf, offset: u16) -> u16 {
    debug_assert!(offset % 2 == 0, "pci_cfg_read16: offset doit être aligné sur 2");
    let word_offset = offset & !0x3;       // arrondir à la dword alignée en dessous
    let byte_shift  = (offset & 0x2) * 8; // 0 ou 16
    let dword = pci_cfg_read32(bdf, word_offset);
    (dword >> byte_shift) as u16
}
```

---

## 10 — Structure des fichiers v10

```
RÉFÉRENCE CANONIQUE :
  ExoOS_Kernel_Types_v10.md    ← types, queue IOMMU CAS, IPC, PCI topology
  ExoOS_Driver_Framework_v10.md ← algorithmes, syscalls, dispatch_irq, do_exit

kernel/
  src/
    memory/
      addr.rs              ← PhysAddr, IoVirtAddr, VirtAddr
      protection.rs        ← PageProtection
    arch/x86_64/
      irq/
        types.rs           ← IrqSourceKind, IrqAckResult, IrqHandler, IrqError
        routing.rs         ← dispatch_irq, ack_irq, sys_irq_register, watchdog
    drivers/
      dma_types.rs         ← DmaDirection, DmaError (sans Overflow)
      pci_types.rs         ← PciBdf, PciError, ClaimError, DeviceClaim (avec bdf)
      pci_topology.rs      ← PciTopology 1024, irq_save, pci_cfg_read16
      dma.rs               ← sys_dma_map
      dma_map_table.rs     ← TempDmaMapping, Drop, revoke_all_for_pid
      device_claims.rs     ← DEVICE_CLAIMS, sys_pci_claim (avec bdf), bdf_of_pid
      device_server_ipc.rs ← toutes les notifications
      iommu/
        fault_handler.rs   ← iommu_fault_isr, iommu_fault_worker_tick
        fault_queue.rs     ← IommuFaultQueue CAS-based strong (FIX-91 + FIX-104)
    process/
      lifecycle.rs         ← do_exit() ordre strict
    time.rs                ← current_time_ms(), BOOT_TSC_KHZ AtomicU64 (FIX-103)
    mmio_cap.rs            ← sys_mmio_map

servers/
  device_server/
    src/
      main.rs              ← init_sequence avec SYS_PCI_SET_TOPOLOGY
      gdi/pci_handle.rs    ← bar_phys
      pci/
        scanner.rs         ← scan_bus_recursive
        link_retraining.rs ← wait_link_retraining (FIX-94 fallback 250ms)
  linux_shim/src/alloc_shim.c ← KmallocHeader (32b, _pad[11])

ARCHIVÉ :
  ExoOS_Kernel_Types_v9.md  ← remplacé (fetch_add orphaned slots, static u64)
  ExoOS_Kernel_Types_v8.md  ← remplacé (static u64, compare_exchange_weak)
  ExoOS_Driver_Framework_v8.md ← remplacé (yield en ISR, EOI Level manquant)

PHASE 9+ :
  ExoOS_SeqLock_Design.md   ← à créer (NMI-safe PciTopology via SeqLock)
```
