EXOOS DRIVER FRAMEWORK
Architecture · Création · Organisation · Erreurs silencieuses
Analyse approfondie + double vérification — ExoOS Phase 8
⚠️  Point de départ : Phase 1 (mémoire) 90% ✅ — Phase 2 (scheduler/IPC) 75% ✅. Les drivers sont Phase 8 minimum. Ce document prépare l'architecture complète pour ne pas improviser quand le moment vient.

Composant	Couche	Priorité	Bloqué par
Kernel IRQ/MMIO/DMA infrastructure	Ring 0	🔴 Fondation	Phase 2 complète (SWAPGS, SMP)
device_server — énumération & lifecycle	Ring 1	🔴 Fondation	Kernel infra + ExoFS (Phase 4)
Generic Driver Interface (GDI trait)	Ring 1	🟠 Architecture	device_server opérationnel
Virtio drivers (VM — priorité 1)	Ring 1	🟠 Premier driver	GDI + device_server
PatchDB statique (boot-critical)	Ring 0 embarqué	🟠 Immédiat	GDI défini
Generic USB HID, NVMe natifs	Ring 1	🟡 Phase 8	PatchDB + GDI
PatchDB dynamique (ExoFS)	Ring 1	🟡 Phase 8	ExoFS monté
Linux Shim Ring 1 (fallback)	Ring 1	🟢 Phase 9	Tous drivers natifs stables
 
1 — Pourquoi le modèle classique a échoué et ce qu'ExoOS change
📋  Cette section justifie chaque décision architecturale. Lire avant de coder quoi que ce soit.

OS / Projet	Approche driver	Raison de l'échec	Ce qu'ExoOS fait différemment
Windows INF + PnP (1995)	Fichier .inf décrit le device → OS applique	INF nécessitait du code Ring 0 arbitraire → vecteur malware massif	Driver en Ring 1 — code Ring 0 = zéro (sauf IRQ routing)
ACPI _DSM (2000)	Hardware se décrit lui-même via BIOS	Constructeurs ont bâclé les tables → Linux a des milliers de lignes de quirks	PatchDB séparée du kernel — quirks sans recompiler
Fuchsia DFv2 (2019)	Driver = composant isolé avec capabilities	Google a pivoté plusieurs fois — écosystème jamais construit	ExoOS = microkernel natif capabilities dès le départ, pas retrofit
OpenBSD autoconf	OS probe chaque bus, drivers déclarent ce qu'ils savent	Statique — base compilée dans le kernel, pas de patch dynamique	PatchDB dynamique chargée depuis ExoFS après montage
Redox OS	Drivers Rust en espace user	Drivers réécrits from scratch — couverture hardware faible après 10 ans	Shim Linux Ring 1 en fallback — 30 ans de drivers accessibles J1

✅  Ce qu'ExoOS peut faire qu'AUCUN des précédents ne pouvait : isolation Ring 1 native + capabilities + PatchDB dynamique + Shim Linux sans risque kernel. Ces 4 pièces ensemble n'ont jamais été assemblées.

2 — Architecture en couches du driver framework
2.1 — Vue d'ensemble des 5 couches
┌─────────────────────────────────────────────────────────────────┐
│  Ring 3 — Userspace                                             │
│  Application ouvre /dev/nvme0 → syscall → vfs_server           │
└─────────────────────────────┬───────────────────────────────────┘
                              │ IPC ExoOS
┌─────────────────────────────▼───────────────────────────────────┐
│  Ring 1 — Couche 5 : Linux Shim (fallback GPU/Wifi/imprimantes) │
│  Ring 1 — Couche 4 : Drivers spécifiques (Intel NIC patch)     │
│  Ring 1 — Couche 3 : Generic Drivers (USB HID, NVMe, Virtio)   │
│  Ring 1 — Couche 2 : device_server (enum, match, lifecycle)    │
└─────────────────────────────┬───────────────────────────────────┘
                              │ Syscalls ExoOS
┌─────────────────────────────▼───────────────────────────────────┐
│  Ring 0 — Couche 1 : Kernel Driver Infrastructure               │
│    • IRQ routing : HW interrupt → IPC vers driver Ring 1        │
│    • MMIO capabilities : accès MMIO délégué via cap             │
│    • DMA management : alloc/free/pin pages DMA                  │
│    • PCI config space : lecture/écriture via syscall           │
└─────────────────────────────┬───────────────────────────────────┘
                              │
┌─────────────────────────────▼───────────────────────────────────┐
│  Couche 0 : Hardware (PCI, USB, ACPI, MMIO)                     │
└─────────────────────────────────────────────────────────────────┘

2.2 — Règle fondamentale : zéro code driver en Ring 0
🔴  RÈGLE DRV-ARCH-01 (absolue) : Le kernel Ring 0 ne contient AUCUNE logique driver. Il fournit uniquement des primitives : livraison d'IRQ, mapping MMIO via capability, allocation DMA pinned. Toute intelligence device est en Ring 1. Un driver qui crashe = un processus Ring 1 qui redémarre. Le kernel ne tombe jamais.

// Ce que Ring 0 fait POUR les drivers :
SYS_IRQ_REGISTER     = 530  // Driver Ring 1 déclare 'je gère IRQ N'
SYS_IRQ_ACK          = 531  // Driver Ring 1 acquitte l'IRQ après traitement
SYS_MMIO_MAP         = 532  // Driver Ring 1 demande mapping phys → virt (avec cap)
SYS_MMIO_UNMAP       = 533  // Driver Ring 1 libère le mapping
SYS_DMA_ALLOC        = 534  // Driver Ring 1 alloue buffer DMA pinned
SYS_DMA_FREE         = 535  // Driver Ring 1 libère buffer DMA
SYS_DMA_SYNC         = 536  // Driver Ring 1 synchronise cache avant/après DMA
SYS_PCI_CFG_READ     = 537  // Lecture PCI config space (u8/u16/u32)
SYS_PCI_CFG_WRITE    = 538  // Écriture PCI config space (contrôlé par capabilities)
SYS_DEV_ENUM_NEXT    = 539  // device_server itère les devices découverts par kernel
 
// ❌ INTERDIT en Ring 0 :
// Toute fonction qui 'comprend' un protocole device (NVMe, USB, Ethernet...)
// Toute structure de données spécifique à un fabricant
// Tout accès MMIO direct au nom d'un driver

3 — Ring 0 : Infrastructure kernel driver
3.1 — IRQ routing : du matériel au driver Ring 1
⚠️  Le chemin interrupt est le composant le plus critique en termes de latence et de correction. Une erreur ici = IRQ storm ou interrupt perdu = device silencieusement cassé.

// kernel/src/arch/x86_64/irq/routing.rs
 
// Modèle : Deferred EOI (fin d'interruption différée)
// Raison : si on envoie EOI avant que le driver Ring 1 ait traité l'IRQ,
// une nouvelle IRQ peut arriver pendant le traitement → storm possible.
 
pub struct IrqRoute {
    irq_line:    u8,
    driver_pid:  u32,             // PID du driver Ring 1
    driver_ep:   IpcEndpoint,     // endpoint IPC pour livraison
    generation:  u64,             // révoqué si driver redémarre
    shared:      bool,            // PCI INTx partagée (plusieurs drivers possible)
    pending_eoi: AtomicBool,      // EOI en attente d'ACK driver
}
 
// Table globale : IRQ 0..255 → Vec<IrqRoute>
// Vec car partage possible (PCI INTx legacy)
static IRQ_TABLE: spin::RwLock<[Option<IrqRoute>; 256]>;
 
// Chemin interrupt (Ring 0, appelé depuis IDT handler) :
pub fn dispatch_irq(irq: u8) {
    let table = IRQ_TABLE.read();
    let Some(route) = &table[irq as usize] else {
        lapic_send_eoi();  // ✅ IRQ orpheline → EOI immédiat
        return;
    };
    // ⚠️  NE PAS envoyer EOI ici — attendre ACK driver
    route.pending_eoi.store(true, Ordering::Release);
    ipc::send_irq_notification(route.driver_ep, irq);  // fast IPC
}
 
// Appelé depuis SYS_IRQ_ACK (driver Ring 1 a fini de traiter) :
pub fn ack_irq(irq: u8, driver_pid: u32) -> Result<(), IrqError> {
    let route = IRQ_TABLE.read();
    // Vérifier que c'est bien CE driver qui possède cet IRQ
    if route[irq as usize].as_ref().map(|r| r.driver_pid) != Some(driver_pid) {
        return Err(IrqError::NotOwner);
    }
    route[irq as usize].as_ref().unwrap().pending_eoi.store(false, Ordering::Release);
    lapic_send_eoi();  // ✅ EOI envoyé seulement après ACK driver
    Ok(())
}

🔴  ERREUR SILENCIEUSE DRV-01 : Si le driver Ring 1 crashe avec pending_eoi=true, le kernel ne reçoit plus jamais d'ACK. L'IRQ line reste masquée en silence. Le device semble 'fonctionner' mais ne livre plus aucun événement. CORRECTION : le kernel watchdog vérifie pending_eoi > 50ms → force EOI + désactive route → init redémarre le driver.

3.2 — MMIO Capabilities
// kernel/src/drivers/mmio_cap.rs
 
pub struct MmioCap {
    phys_base:  PhysAddr,    // adresse physique MMIO
    size:       usize,       // taille en bytes
    virt_base:  VirtAddr,    // adresse virtuelle dans l'espace du driver
    rights:     MmioRights,  // READ | WRITE (jamais EXEC pour du MMIO)
    owner_pid:  u32,         // PID du driver propriétaire
    generation: u64,         // invalide si driver redémarre
    flags:      MmioFlags,   // UC (Uncacheable) obligatoire, jamais WC pour MMIO control
}
 
// SYS_MMIO_MAP : kernel vérifie que phys_base est dans une région MMIO légale
// (pas dans la RAM kernel, pas dans les zones protégées ACPI)
pub fn sys_mmio_map(phys: PhysAddr, size: usize, flags: MmioFlags,
                    requesting_pid: u32) -> Result<VirtAddr, MmioError> {
    // 1. Vérifier que phys n'est PAS dans la RAM physique (DMA-06)
    if memory_map::is_ram_region(phys, size) {
        return Err(MmioError::PhysIsRam);  // ❌ Empêche mapping RAM comme MMIO
    }
    // 2. Vérifier que la région n'est pas déjà mappée par un autre driver
    if mmio_table::is_claimed(phys, size) {
        return Err(MmioError::AlreadyClaimed);
    }
    // 3. Allouer une VMA dans l'espace du driver Ring 1
    let virt = process::map_mmio_in_driver(requesting_pid, phys, size,
        PAGE_FLAGS_MMIO)?;  // PTE_PRESENT | PTE_WRITABLE | PTE_CACHE_DISABLE | PTE_NO_EXEC
    // 4. Enregistrer la cap
    let generation = process::get_generation(requesting_pid);
    mmio_table::register(phys, size, requesting_pid, generation, virt);
    Ok(virt)
}
 
// Appelé automatiquement à la mort du processus driver :
pub fn revoke_all_mmio_for_process(pid: u32) {
    for cap in mmio_table::caps_of_pid(pid) {
        process::unmap_vma(pid, cap.virt_base, cap.size);
        mmio_table::remove(cap.phys_base);
    }
}

🔴  ERREUR SILENCIEUSE DRV-02 : Si revoke_all_mmio_for_process() n'est pas appelé à la mort du driver, les pages MMIO restent mappées. Si init alloue un nouveau PID qui tombe sur le même numéro (recycling), le nouveau processus hérite de l'accès MMIO de l'ancien driver. Exploit trivial. CORRECTION : revoke appelé dans do_exit() AVANT que le PID soit recyclé.

3.3 — DMA management
// kernel/src/drivers/dma.rs
 
pub struct DmaBuffer {
    virt:      VirtAddr,       // adresse virtuelle dans l'espace du driver
    phys:      PhysAddr,       // adresse physique pour le device
    size:      usize,
    direction: DmaDirection,   // ToDevice | FromDevice | Bidirectional
    ref_count: AtomicU32,      // libéré seulement quand ref_count == 0
    owner_pid: u32,
}
 
// RÈGLE DMA-01 : pages DMA sont PINNED — jamais swapées
// Utilise buddy zone DMA (< 16 MiB) ou DMA32 (< 4 GiB)
pub fn sys_dma_alloc(size: usize, dir: DmaDirection,
                     requesting_pid: u32) -> Result<DmaBuffer, DmaError> {
    // Arrondir à page size
    let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    // Allouer depuis zone DMA32 — frames physiquement contigus
    let phys = buddy::alloc_contiguous(pages, Zone::Dma32)?;
    // Marquer comme PINNED dans les flags de page
    for i in 0..pages {
        page_flags::set(phys + i * PAGE_SIZE, PF_PINNED | PF_DMA);
    }
    // Mapper dans l'espace du driver
    let virt = process::map_dma_in_driver(requesting_pid, phys, pages * PAGE_SIZE)?;
    let buf = DmaBuffer { virt, phys, size, direction: dir,
        ref_count: AtomicU32::new(1), owner_pid: requesting_pid };
    dma_table::register(buf.phys, buf.clone());
    Ok(buf)
}
 
// ✅ SYS_DMA_SYNC : invalide les caches CPU AVANT que le device lise (ToDevice)
// ✅ SYS_DMA_SYNC : invalide les caches CPU APRÈS que le device écrit (FromDevice)
// ❌ INTERDIT : accéder au buffer DMA côté CPU sans SYS_DMA_SYNC préalable
🔴  ERREUR SILENCIEUSE DRV-03 (DMA-after-free) : Driver alloue DMA buffer, programme l'adresse physique dans le NIC, driver crashe → DMA buffer libéré → buddy le réalloue pour le heap kernel → NIC continue d'écrire vers cette adresse physique → heap kernel corrompu silencieusement. CORRECTION : ref_count décrémenté à la mort du driver, buffer libéré UNIQUEMENT quand ref_count == 0. Le device doit être arrêté (PCI bus master disable) avant que le buffer soit libéré.

4 — device_server Ring 1 : Énumération, matching, lifecycle
4.1 — Rôle et architecture
Le device_server est le cœur du driver framework. Il est responsable de découvrir les devices, charger les bons drivers, et superviser leur cycle de vie. Il tourne en Ring 1, PID fixe (probablement PID 5 après init/ipc_broker/vfs_server/crypto_server).

// servers/device_server/src/main.rs
 
// Séquence de démarrage device_server :
// 1. init_pci_scanner()     → scanne PCI bus 0..255 via SYS_PCI_CFG_READ
// 2. init_acpi_devices()    → ACPI _HID/_CID depuis tables parsées au boot
// 3. init_usb_wait()        → attend que XHCI driver Ring 1 soit actif
//    (XHCI est lui-même un device PCI découvert à l'étape 1)
// 4. load_static_patchdb()  → PatchDB embarquée (pas besoin d'ExoFS)
// 5. match_and_spawn()      → pour chaque device, charge le driver Ring 1
// 6. load_dynamic_patchdb() → charge PatchDB complète depuis ExoFS (si monté)
// 7. event_loop()           → écoute hotplug, crash, restart
 
pub struct DeviceRecord {
    id:          DeviceId,        // (vendor_id, device_id, class, subclass)
    bus:         BusType,         // Pci { bus, slot, func } | Usb { addr } | Acpi { hid }
    description: &'static str,   // 'Intel I225 Ethernet Controller'
    driver_pid:  Option<u32>,     // None si pas encore chargé
    state:       DeviceState,     // Unbound | Probing | Active | Failed | Suspended
    mmio_caps:   Vec<MmioCapInfo>,
    dma_bufs:    Vec<DmaHandle>,
    irq_lines:   Vec<u8>,
}

4.2 — PCI Scanner
// servers/device_server/src/pci/scanner.rs
 
// Scan PCI : lecture config space via SYS_PCI_CFG_READ
// Pas besoin de MMIO direct en Ring 0 — le syscall abstrait ça
 
pub fn scan_pci_bus() -> Vec<PciDevice> {
    let mut devices = Vec::new();
    for bus in 0u8..=255 {
        for slot in 0u8..32 {
            for func in 0u8..8 {
                let vendor = pci_read16(bus, slot, func, 0x00);
                if vendor == 0xFFFF { continue; } // Slot vide
                let device    = pci_read16(bus, slot, func, 0x02);
                let class     = pci_read8 (bus, slot, func, 0x0B);
                let subclass  = pci_read8 (bus, slot, func, 0x0A);
                let prog_if   = pci_read8 (bus, slot, func, 0x09);
                let revision  = pci_read8 (bus, slot, func, 0x08);
                let hdr_type  = pci_read8 (bus, slot, func, 0x0E);
                devices.push(PciDevice { bus, slot, func,
                    vendor, device, class, subclass, prog_if, revision });
                // Si header type 1 (bridge) → scan bus secondaire
                if hdr_type & 0x7F == 0x01 {
                    let secondary = pci_read8(bus, slot, func, 0x19);
                    // Récursion sur secondary bus (borné à profondeur 8)
                }
                if func == 0 && (hdr_type & 0x80 == 0) { break; } // Non multi-func
            }
        }
    }
    devices
}

4.3 — Driver lifecycle et supervision
⚠️  ERREUR SILENCIEUSE DRV-04 : Ordre de démarrage des drivers. Si USB HID driver est spawné avant que XHCI soit prêt, le probe échoue silencieusement et le clavier ne fonctionne jamais, même après que XHCI soit actif. Le device_server doit gérer les dépendances topologiques.

// servers/device_server/src/lifecycle.rs
 
// Graphe de dépendances des drivers
// USB HID dépend de XHCI (qui dépend de PCI discovery)
// NVMe dépend de PCI discovery
// Ethernet dépend de PCI discovery
 
pub fn match_and_spawn(devices: &[DeviceRecord]) {
    // Phase 1 : drivers sans dépendances (PCI directs)
    let pci_ready: Vec<_> = devices.iter()
        .filter(|d| d.bus.is_pci() && !d.needs_parent_driver())
        .collect();
    for dev in pci_ready { spawn_driver(dev); }
    
    // Phase 2 : attendre que les controllers soient prêts (XHCI, AHCI...)
    wait_for_driver_class(PciClass::UsbController);
    wait_for_driver_class(PciClass::StorageController);
    
    // Phase 3 : devices USB, NVMe etc.
    let secondary: Vec<_> = devices.iter()
        .filter(|d| d.needs_parent_driver())
        .collect();
    for dev in secondary { spawn_driver(dev); }
}
 
// Supervision : si un driver crashe, init le redémarre via SIGCHLD
// device_server reçoit la notification et relance le driver avec état reset
pub fn on_driver_crash(pid: u32) {
    let dev = find_device_by_driver_pid(pid);
    // 1. Kernel a déjà révoqué MMIO caps et DMA buffers (do_exit())
    // 2. Kernel a déjà désactivé le bus mastering PCI (DRV-04)
    // 3. device_server marque le device Failed
    dev.state = DeviceState::Failed;
    // 4. Attendre 1s (backoff) puis respawn
    schedule_respawn(dev, Duration::from_secs(1));
}
🔴  ERREUR SILENCIEUSE DRV-05 (Ghost IRQ registration) : Driver A enregistre IRQ 11 → Driver A crashe → init le redémarre → Driver A réenregistre IRQ 11. Si le kernel n'a pas révoqué l'ancienne registration, il y a maintenant DEUX handlers pour IRQ 11. L'IRQ est livrée deux fois → device reçoit commandes en double. CORRECTION : SYS_IRQ_REGISTER vérifie par génération de processus, pas par PID seul.

5 — Generic Driver Interface (GDI)
5.1 — Le trait central
// servers/device_server/src/gdi/mod.rs  (partagé par tous les drivers)
// Inclus via le crate 'exo-driver-sdk' — publié dans le workspace
 
pub trait ExoDriver: Send + Sync {
    /// Retourne les IDs que ce driver peut gérer.
    /// Appelé au boot sans allocations.
    fn supported_ids() -> &'static [DeviceMatch] where Self: Sized;
 
    /// Probe : ce device est-il gérable par ce driver ?
    /// Résultat : Yes (confiance), Maybe (fallback possible), No.
    fn probe(&self, info: &DeviceInfo) -> ProbeResult;
 
    /// Attach : initialiser le device avec les ressources allouées.
    fn attach(&mut self, handle: DeviceHandle) -> Result<(), DriverError>;
 
    /// IRQ handler : appelé via IPC quand une IRQ est livrée.
    /// DOIT retourner Handled ou NotMine dans les 5ms.
    /// DOIT appeler handle.irq_ack() avant de retourner Handled.
    fn handle_irq(&mut self, irq: u8) -> IrqResult;
 
    /// Detach : nettoyage (appelé avant le crash recovery aussi).
    fn detach(&mut self);
 
    /// Power management
    fn suspend(&mut self) -> Result<(), DriverError> { Ok(()) }
    fn resume (&mut self) -> Result<(), DriverError> { Ok(()) }
}
 
// DeviceMatch : critères de correspondance (ordre de priorité)
pub enum DeviceMatch {
    Exact    { vendor: u16, device: u16 },           // Priorité 1 — meilleur match
    ClassSub { class: u8, sub: u8 },                 // Priorité 2 — driver générique
    Class    { class: u8 },                          // Priorité 3 — fallback
    AcpiHid  { hid: &'static str },                  // ACPI _HID
}
 
// IrqResult — le driver DOIT indiquer si l'IRQ était pour lui
// Critique pour les IRQ partagées PCI INTx
pub enum IrqResult {
    Handled,    // IRQ traitée, EOI peut être envoyé
    NotMine,    // IRQ pas pour ce driver (IRQ partagée)
    Error(DriverError),  // Erreur → device_server log + possible restart
}

5.2 — DeviceHandle : les ressources accessibles au driver
// DeviceHandle est passé à attach() — contient toutes les ressources du device
pub struct DeviceHandle {
    pub device_id:  DeviceId,
    pub pci:        Option<PciHandle>,     // BAR access, config read/write
    pub mmio:       Vec<MmioRegion>,       // régions MMIO déjà mappées
    pub irq:        Vec<IrqHandle>,        // lignes IRQ disponibles
    pub dma:        DmaAllocator,          // allocateur DMA pour ce driver
    pub patch:      Option<&'static DevicePatch>, // quirks depuis PatchDB
    pub log:        DriverLogger,          // log vers journal ExoOS
}
 
impl PciHandle {
    // BAR probe : lit la taille du BAR de manière sûre
    // ✅ Sauvegarde la valeur originale, restaure après lecture taille
    pub fn bar_size(&self, bar_idx: u8) -> usize { ... }
 
    // Lecture/écriture config space — gatée par capability
    pub fn cfg_read32(&self, offset: u16) -> u32 { ... }
    pub fn cfg_write32(&self, offset: u16, val: u32) { ... }
 
    // Activer/désactiver bus mastering (DMA depuis le device)
    pub fn enable_bus_master(&self) { ... }
    pub fn disable_bus_master(&self) { ... }  // ✅ Appelé automatiquement à detach()
}
⚠️  ERREUR SILENCIEUSE DRV-06 (BAR size miscalculation) : Pour lire la taille d'un BAR PCI, il faut écrire 0xFFFFFFFF dans le registre BAR, lire la réponse, recalculer. Si on oublie de sauvegarder et restaurer la valeur originale, le BAR est écrasé et le device ne répond plus. bar_size() doit TOUJOURS faire save→write→read→restore.

6 — PatchDB : patches device sans recompiler le kernel
📋  La PatchDB est ce qui différencie ExoOS de Redox OS. Redox doit réécrire chaque driver. ExoOS a un driver générique + 20 lignes de patch par constructeur. La PatchDB peut être mise à jour par la communauté sans toucher au kernel.

6.1 — Deux niveaux de PatchDB
Niveau	Localisation	Chargé quand	Format	Couverture
PatchDB statique	Embarquée dans device_server binary	Boot — avant montage ExoFS	postcard (binaire no_std)	Devices critiques boot : NVMe, AHCI, XHCI, VirtIO, GPU framebuffer basique
PatchDB dynamique	ExoFS — objet content-addressed	Après montage ExoFS (Phase 4 complete)	postcard ou TOML compilé	Tous les autres devices : Ethernet, Wifi, Audio, Bluetooth, etc.

6.2 — Format d'un patch
// exo-driver-sdk/src/patchdb/mod.rs
 
#[derive(Serialize, Deserialize)]  // postcard
pub struct DevicePatch {
    pub vendor_id:        u16,
    pub device_id:        u16,
    pub subsystem_vendor: u16,   // 0x0000 = wildcard
    pub subsystem_device: u16,   // 0x0000 = wildcard
    pub revision_min:     u8,    // 0x00 = any
    pub revision_max:     u8,    // 0xFF = any
    pub quirks:           &'static [Quirk],
    pub init_sequence:    &'static [RegWrite],
    pub override_class:   Option<u8>,   // forcer une class PCI différente
}
 
#[derive(Serialize, Deserialize)]
pub enum Quirk {
    DelayAfterResetMs(u32),         // Intel NIC : 10ms après reset
    NoEepromCheck,                  // Certains Realtek : skip EEPROM
    DisableMsi,                     // Forcer INTx (MSI buggé)
    ForceLink1000Full,              // NIC : forcer autoneg
    MmioBarIndex(u8),              // Override : quel BAR contient le MMIO
    DmaAlignmentBytes(usize),       // DMA alignment requis
    PciPowerStateD0Delay(u32),      // ms à attendre après power-on
    XhciVendorSpecificInit(u32),    // Registre init spécifique XHCI
}
 
#[derive(Serialize, Deserialize)]
pub struct RegWrite {
    pub offset: u32,   // offset dans le BAR MMIO
    pub value:  u32,
    pub mask:   u32,   // 0xFFFFFFFF = write complet, sinon read-modify-write
    pub delay_us: u32, // délai après écriture (0 = aucun)
}
 
// Exemple : patch Intel I225 Ethernet (2.5Gbps)
// 20 lignes remplacent un driver spécifique entier :
pub static PATCH_INTEL_I225: DevicePatch = DevicePatch {
    vendor_id: 0x8086, device_id: 0x15F3,
    subsystem_vendor: 0x0000, subsystem_device: 0x0000,
    revision_min: 0x00, revision_max: 0xFF,
    quirks: &[Quirk::DelayAfterResetMs(10), Quirk::ForceLink1000Full],
    init_sequence: &[
        RegWrite { offset: 0x00100, value: 0x04000000, mask: 0xFFFFFFFF, delay_us: 0 },
        RegWrite { offset: 0x00018, value: 0x000C0000, mask: 0xFFFFFFFF, delay_us: 100 },
    ],
    override_class: None,
};

6.3 — Résolution patch : (vendor, device) → patch applicable
// Résolution en O(1) via hashmap statique au boot
// La clé est (vendor_id, device_id) — sous-clé subsystem si ambiguïté
 
pub struct PatchRegistry {
    // Lookup rapide : (vendor, device) → index dans PATCHES[]
    exact:   HashMap<(u16,u16), usize, SipHasher>,
    // Fallback : class → index dans CLASS_PATCHES[]
    class:   [Option<usize>; 256],
}
 
impl PatchRegistry {
    pub fn lookup(&self, dev: &PciDevice) -> Option<&DevicePatch> {
        // 1. Match exact vendor+device (priorité maximale)
        if let Some(&idx) = self.exact.get(&(dev.vendor, dev.device)) {
            let p = &PATCHES[idx];
            // Vérifier subsystem si patch le requiert
            if p.subsystem_vendor != 0 && p.subsystem_vendor != dev.subsystem_vendor {
                // Pas de match subsystem — chercher un patch moins spécifique
            } else {
                return Some(p);
            }
        }
        // 2. Fallback : patch de classe
        if let Some(idx) = self.class[dev.class as usize] {
            return Some(&CLASS_PATCHES[idx]);
        }
        None  // Pas de patch — driver générique nu
    }
}

7 — Drivers priorité 1 : Virtio (couverture VMs complète)
✅  Virtio est la priorité absolue. Quatre drivers Virtio couvrent 100% des VMs QEMU : block (stockage), net (réseau), gpu (framebuffer), input (clavier/souris). Avec ces 4 drivers, ExoOS est utilisable dans une VM sans un seul driver natif.

7.1 — Crate virtio-drivers (rcore-os) — stratégie fork
Aspect	État	Action requise
Crate existante	rcore-os/virtio-drivers — Rust no_std, active	Forker dans servers/device_server/vendor/virtio-drivers/
Couverture	Block, Net, GPU, Input, Console, Sound	Utiliser Block + Net + GPU + Input en priorité
HAL trait	Doit être implémenté pour ExoOS (DMA, MMIO)	Implémenter VirtioHal pour ExoOS : SYS_DMA_ALLOC + SYS_MMIO_MAP
no_std	Oui — compatible Ring 1 (std disponible en Ring 1 mais no_std reste propre)	Aucune modification nécessaire
MSI-X	Supporté dans le crate	Activer — MSI-X préféré à INTx pour Virtio

// servers/device_server/src/drivers/virtio/hal.rs
// Implémentation du HAL trait pour ExoOS
 
pub struct ExoVirtioHal;
 
unsafe impl Hal for ExoVirtioHal {
    fn dma_alloc(pages: usize, direction: BufferDirection)
        -> (PhysAddr, NonNull<u8>) {
        // Appel SYS_DMA_ALLOC → kernel alloue pages contigus zone DMA32
        let buf = exo_syscall::dma_alloc(pages * PAGE_SIZE,
            direction.into()).expect('DMA alloc failed');
        (buf.phys, NonNull::new(buf.virt as *mut u8).unwrap())
    }
 
    unsafe fn dma_dealloc(paddr: PhysAddr, _vaddr: NonNull<u8>, pages: usize)
        -> i32 {
        exo_syscall::dma_free(paddr, pages * PAGE_SIZE);
        0
    }
 
    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, size: usize) -> NonNull<u8> {
        // Appel SYS_MMIO_MAP → kernel mappe avec PAGE_FLAGS_MMIO (UC + NX)
        let virt = exo_syscall::mmio_map(paddr, size,
            MmioFlags::UNCACHEABLE).expect('MMIO map failed');
        NonNull::new(virt as *mut u8).unwrap()
    }
 
    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection)
        -> PhysAddr {
        // SYS_DMA_SYNC : invalider caches avant partage avec device
        exo_syscall::dma_sync(buffer.as_ptr() as PhysAddr,
            buffer.len(), direction.into());
        virt_to_phys(buffer.as_ptr() as usize)
    }
}

7.2 — Ordre de chargement des drivers Virtio
Ordre	Driver	Classe PCI	Impact immédiat
1	virtio-blk	0x01/0x00 Storage	Stockage VM → ExoFS peut être monté
2	virtio-net	0x02/0x00 Ethernet	Réseau VM → TCP/IP disponible
3	virtio-gpu	0x03/0x80 Display	Framebuffer → affichage graphique
4	virtio-input	0x09/0x00 Input	Clavier/souris → interaction utilisateur

8 — Linux Shim Ring 1 : 30 ans de drivers accessibles
8.1 — Principe et surface d'API
📋  Le shim n'est PAS du binaire compat (charger un .ko Linux). C'est un ensemble de stubs C qui traduisent les appels kernel Linux vers les primitives ExoOS. Le driver Linux est recompilé contre ces stubs. Le résultat tourne en Ring 1.

La surface d'API Linux utilisée par un driver typique est étonnamment petite. Environ 200-300 fonctions couvrent 95% des drivers :

Catégorie Linux	Fonctions principales	Traduction ExoOS	Complexité
Mémoire	kmalloc/kfree, vmalloc/vfree, kzalloc, devm_kmalloc	exo_malloc/exo_free (heap Ring 1) + header taille pour kfree sans size	Faible — std::alloc en Ring 1
MMIO	ioread32/iowrite32, readl/writel, ioremap/iounmap	SYS_MMIO_MAP + pointeur UC + read_volatile/write_volatile	Faible — mapping direct
DMA	dma_alloc_coherent, dma_free_coherent, dma_map_single, dma_sync_single	SYS_DMA_ALLOC / SYS_DMA_FREE / SYS_DMA_SYNC	Moyenne — pinning à gérer
IRQ	request_irq, free_irq, enable_irq, disable_irq	SYS_IRQ_REGISTER / SYS_IRQ_ACK + thread IPC IRQ handler	Moyenne — ACK explicite requis
PCI	pci_read_config_*, pci_enable_device, pci_request_regions, pci_set_master	SYS_PCI_CFG_READ/WRITE + SYS_MMIO_MAP pour BARs	Faible
Synchronisation	spin_lock/unlock, mutex_lock/unlock, completion_wait/complete	std::sync::Mutex, Condvar (Ring 1 a accès à std)	Très faible
Timers	mod_timer, del_timer, msleep, udelay	std::thread::sleep, IPC timer (hrtimer via device_server)	Faible
Work queues	schedule_work, INIT_WORK, flush_work	std::thread::spawn + channel (Ring 1 a des threads)	Faible
Device model	device_register, class_create, driver_register	Enregistrement dans device_server via IPC	Moyenne — mapping conceptuel

🔴  ERREUR SILENCIEUSE DRV-07 (kfree size mismatch) : Linux kfree() n'a pas besoin de la taille — l'allocateur Linux la connaît. ExoOS SLUB a besoin de la classe de taille pour free. Le shim kmalloc() DOIT préfixer chaque allocation avec un header stockant la taille. Sinon : corruption SLUB silencieuse.

// servers/linux_shim/src/alloc_shim.c
// Header caché devant chaque allocation kmalloc
 
typedef struct { size_t real_size; uint8_t data[]; } KmallocHeader;
 
void* kmalloc(size_t size, gfp_t flags) {
    KmallocHeader* hdr = exo_malloc(sizeof(KmallocHeader) + size);
    if (!hdr) return NULL;
    hdr->real_size = size;
    return hdr->data;  // Retourne pointeur APRÈS le header
}
 
void kfree(const void* ptr) {
    if (!ptr) return;
    KmallocHeader* hdr = (KmallocHeader*)ptr - 1;
    // ✅ On connaît la taille grâce au header
    exo_free(hdr, sizeof(KmallocHeader) + hdr->real_size);
}
 
// ⚠️  GFP flags Linux sont ignorés — en Ring 1 tout est 'normal' memory
// GFP_KERNEL = GFP_DMA = GFP_ATOMIC → tous traités pareil (pas d'ISR en Ring 1)
// Exception : GFP_DMA → appel SYS_DMA_ALLOC au lieu de exo_malloc

9 — Catalogue complet des erreurs silencieuses driver
📋  Ces erreurs ne crashent pas immédiatement. Elles produisent des comportements incorrects qui se manifestent sous charge, sur hardware réel, ou après plusieurs heures. Chacune a une correction précise.

ID	Module	Erreur silencieuse	Symptôme tardif	Correction
DRV-01	irq/routing.rs	Driver crashe avec pending_eoi=true IRQ line reste masquée	Device 'actif' mais ne livre plus aucun événement. Invisible en QEMU.	Watchdog kernel : pending_eoi > 50ms → force EOI + disable route + respawn
DRV-02	mmio_cap.rs	MMIO non révoqué à la mort du driver PID recyclé → nouveau proc hérite accès	Nouveau processus accède MMIO d'un ancien device — exploit trivial	revoke_all_mmio() dans do_exit() AVANT recyclage PID
DRV-03	dma.rs	DMA after free : device continue DMA après que le buffer est libéré	Heap kernel corrompu silencieusement Crash différé aléatoire	ref_count DMA — libéré seulement quand device et driver ont release()
DRV-04	dma.rs + pci	Bus mastering PCI non désactivé après crash driver	Device initie DMA arbitrairement sans driver pour le contrôler	pci_disable_bus_master() dans revoke_all_mmio() automatiquement
DRV-05	irq/routing.rs	Double registration IRQ après restart Ghost handler de l'ancien driver	IRQ livrée deux fois → commandes en double → device corrompu	Génération dans IrqRoute SYS_IRQ_REGISTER révoque l'ancien
DRV-06	pci/bar.rs	BAR size probe sans save/restore Écrase la valeur originale du BAR	Device ne répond plus Silencieux jusqu'au prochain accès MMIO	bar_size() : save→write→read→restore Toujours via PciHandle.bar_size()
DRV-07	linux_shim/alloc	kfree() sans connaissance de la taille Corruption SLUB	Crash kernel différé Allocation suivante retourne données corrompues	Header hidden devant kmalloc() kfree lit le header pour la taille
DRV-08	irq/routing.rs	IRQ partagée (PCI INTx) sans retour NotMine si pas pour soi	IRQ non acquittée → storm infini CPU 100% silencieux	handle_irq() doit toujours retourner Handled ou NotMine — jamais void
DRV-09	pci/msi.rs	MSI-X table dans le même BAR que les registres de contrôle	Driver écrit accidentellement dans MSI-X table → wrong IRQ vectors	MSI-X table = capability séparée Mappée read-only depuis driver
DRV-10	drivers/lifecycle	Driver spawné avant son controller Ex: USB HID avant XHCI ready	Probe échoue silencieusement Clavier ne fonctionne jamais	Topological sort des dépendances wait_for_driver_class() bloquant

10 — Crates : utiliser / forker / écrire from scratch
⚠️  Règle absolue : NE JAMAIS introduire une crate dans Ring 0 sans audit complet. Les crates pour les drivers tournent en Ring 1 (std disponible) — contraintes moins strictes mais toujours pas de unsafe non audité.

Crate	Source	no_std	Ring	Action	Raison
virtio-drivers	rcore-os/virtio-drivers	Oui	1	✅ Forker	Meilleure implem Virtio Rust HAL trait à implémenter pour ExoOS
pci-types	rust-osdev/pci-types	Oui	0+1	✅ Utiliser	Types PCI config space Pas de logique — juste des types
acpi	rust-osdev/acpi	Oui	0	✅ Déjà en use	_DSM/_CRS nécessaires pour drivers ACPI Déjà utilisé pour RSDP/MADT
postcard	jamesmunns/postcard	Oui	0+1	✅ Déjà en use	PatchDB format binaire Déjà dans le workspace
smoltcp	smoltcp-rs/smoltcp	Oui	1	✅ Déjà en use	TCP/IP stack — net_stack server Déjà prévu dans servers/
usb-device	rust-embedded	Oui	1	❌ Ne pas utiliser	C'est le côté DEVICE (périphérique) Pas le côté HOST (controller)
xhci	pas de bonne crate	—	1	✍️ Écrire from scratch	Aucun crate Rust XHCI host mature Porter depuis Linux ou Redox
toml / toml_edit	std requis	Non	—	❌ Ring 0	PatchDB dynamique = postcard toml seulement pour tooling offline
linux-driver-shim	inexistant	—	1	✍️ Écrire (~2000 lignes C)	200-300 stubs C bien définis Surface API Linux limitée
pci-scanner	inexistant	—	1	✍️ Écrire (~300 lignes)	Logique de scan PCI simple Pas de crate no_std mature
device-matching	inexistant	—	1	✍️ Écrire (~400 lignes)	Logique de match vendor/class Spécifique ExoOS

10.1 — Fork virtio-drivers : travail concret
# Dans le workspace ExoOS :
servers/device_server/vendor/virtio-drivers/   ← fork local
 
# Modifications requises dans le fork :
# 1. Supprimer dépendance sur 'log' crate → utiliser DriverLogger ExoOS
# 2. Implémenter Hal trait dans servers/device_server/src/drivers/virtio/hal.rs
# 3. Ajouter feature 'exo-os' qui active l'implem HAL ExoOS
# 4. Tester sur QEMU -machine q35 avec -device virtio-blk-pci
 
# Cargo.toml servers/device_server :
[dependencies]
virtio-drivers = { path = 'vendor/virtio-drivers', features = ['blk','net','gpu','input'] }

10.2 — Écriture du Linux Shim : ordre et priorités
Priorité	Module shim	Lignes estimées	Drivers débloqués
1 — Immédiat	alloc_shim.c (kmalloc/kfree/vmalloc)	~150 lignes	Tous les drivers Linux
1 — Immédiat	mmio_shim.c (ioread/iowrite/ioremap)	~100 lignes	Tous les drivers PCI MMIO
2 — Court terme	irq_shim.c (request_irq/free_irq)	~200 lignes	Drivers qui utilisent les IRQ
2 — Court terme	dma_shim.c (dma_alloc_coherent)	~200 lignes	NIC, storage, USB host
3 — Moyen terme	pci_shim.c (pci_enable, regions)	~300 lignes	Drivers PCI génériques
3 — Moyen terme	sync_shim.c (spinlock, mutex)	~150 lignes	Tous — mais std::sync suffit souvent
4 — Plus tard	work_shim.c (workqueue, delayed_work)	~200 lignes	Drivers avec tâches différées
4 — Plus tard	timer_shim.c (mod_timer, del_timer)	~150 lignes	Drivers avec timers logiciels

11 — Double analyse : vérification des erreurs de conception
🔬  Méthode : PASSE 1 = analyse initiale de l'architecture. PASSE 2 = critique de la PASSE 1 pour trouver les hypothèses fausses et les angles morts. Ce que la PASSE 2 trouve, la PASSE 1 ne l'aurait jamais détecté.

11.1 — PASSE 1 : Problèmes identifiés dans l'architecture
#	Problème identifié	Impact	Gravité
P1-A	Latence IRQ via IPC : Ring 0 reçoit l'IRQ, envoie IPC à Ring 1, Ring 1 traite, Ring 1 ACK, Ring 0 envoie EOI. Aller-retour IPC potentiellement 5-20µs.	Audio temps-réel impossible (< 1ms requis). Réseau acceptable (100µs toléré).	⚠️ Moyen
P1-B	Chicken-and-egg PatchDB/NVMe : La PatchDB dynamique est dans ExoFS. Mais pour monter ExoFS, il faut le driver NVMe. Si NVMe a besoin d'un patch dans la PatchDB dynamique, il ne peut pas l'avoir avant d'être chargé.	Certains SSD NVMe ne fonctionneraient jamais.	🔴 Critique
P1-C	Scan PCI depuis Ring 1 via SYS_PCI_CFG_READ : 256 bus × 32 slots × 8 fonctions × 2 lectures = 131072 appels syscall. À ~1µs par syscall = 130ms de boot.	Boot trop lent si scan complet.	⚠️ Moyen
P1-D	Linux Shim GFP_ATOMIC ignoré : en Ring 1 il n'y a pas d'ISR, donc GFP_ATOMIC n'a pas de sens. Mais certains drivers Linux appellent kmalloc(GFP_ATOMIC) depuis des spin_lock. En Ring 1, un spin_lock qui bloque est acceptable mais si le driver s'attend à une allocation non-bloquante, un délai peut casser une invariante.	Driver Linux se comporte incorrectement avec certaines allocations.	⚠️ Moyen
P1-E	device_server singleton et SPOF : Si device_server crashe, tous les drivers perdent leur superviseur. Les devices déjà actifs continuent (Ring 1 processes indépendants) mais aucun nouveau device ne peut être enregistré et aucun restart de driver n'est possible.	Hotplug cassé après crash device_server.	🔴 Critique

11.2 — PASSE 2 : Corrections et réfutations de PASSE 1
Problème PASSE 1	Hypothèse fausse détectée	Correction validée	Résidu
P1-A — Latence IRQ IPC	FAUX d'assumer que tout driver a besoin de latence < 1ms. GPU, NIC, storage : 100µs amplement suffisant. Seul l'audio temps-réel est problématique.	Pour audio RT : thread IRQ dédié Ring 1 avec priorité SCHED_FIFO. Le thread dort sur IPC bloquant → réveil < 10µs. Fuchsia prouve que c'est faisable.	Audio RT difficile mais pas impossible. Le scheduler CFS/RT est déjà implémenté en Phase 2.
P1-B — Chicken-and-egg PatchDB/NVMe	FAUX d'assumer que tous les patches NVMe sont dans la PatchDB dynamique. La PatchDB STATIQUE embarquée dans device_server couvre exactement ce cas.	PatchDB statique contient : tous patches NVMe/AHCI connus, tous patches XHCI, tous patches VirtIO. PatchDB dynamique = seulement les devices non-critiques (Ethernet exotique, Wifi, etc.)	Risque résiduel : NVMe avec quirk inconnu au moment de la compilation. Solution : feature flag 'safe_mode' qui ignore les patches inconnus et tente le generic driver.
P1-C — Scan PCI 130ms	FAUX de penser qu'il faut scanner 256 bus. En pratique : bus 0 uniquement pour la plupart des systèmes. ACPI MCFG table liste les bus PCI valides.	Parser ACPI MCFG (déjà disponible depuis Phase 1) pour obtenir la liste des bus valides. Typiquement 1-4 bus → scan × 64 = 2048 syscalls → 2ms. Acceptable.	Systèmes enterprise avec 16+ buses PCI : scan peut prendre 16ms. Acceptable pour un serveur.
P1-D — GFP_ATOMIC Ring 1	FAUX de supposer que GFP_ATOMIC est un problème. En Ring 1, il n'y a pas de contexte ISR — spin_lock ne désactive pas les IRQ. Donc GFP_ATOMIC = GFP_KERNEL en Ring 1. Le comportement est PLUS permissif, pas moins.	GFP_ATOMIC mappé à GFP_KERNEL dans alloc_shim.c. La seule différence comportementale : pas de retry d'allocation en Ring 1. Si kmalloc retourne NULL, le driver doit gérer. Ce qu'il fait déjà.	Drivers qui ignorent le NULL retour de kmalloc crasheront. Ce sont des bugs dans ces drivers, pas dans ExoOS.
P1-E — device_server SPOF	FAUX de penser que device_server doit être un singleton. Il peut être supervisé par init avec restart automatique. Les drivers Ring 1 actifs sont des processus indépendants — ils ne meurent pas si device_server crashe.	init supervise device_server avec SIGCHLD. Si device_server crashe : restart en < 1s. Pendant le restart : drivers actifs continuent, hotplug suspendu. État restauré : device_server re-scanne PCI et retrouve les drivers déjà actifs par leur PID (enregistrés dans ipc_broker).	device_server doit persister son état dans ipc_broker pour le retrouver au restart. Travail supplémentaire mais faisable.

✅  Résultat de la double analyse : les 5 problèmes PASSE 1 ont tous des corrections valides. Aucun n'est architectural — ce sont des détails d'implémentation. L'architecture en couches Ring 0/1/3 avec capabilities reste la bonne fondation.

12 — Ordre d'implémentation : du concret au complet
Étape	Module	Prérequis	Livrable	Valide si
8.1	Syscalls driver Ring 0 (SYS_IRQ_REGISTER..SYS_DMA_SYNC)	Phase 2 complète (SWAPGS, SMP actif)	10 nouveaux syscalls 530-539 dans kernel/src/syscall/handlers/	Test : Ring 1 peut lire PCI config via SYS_PCI_CFG_READ sans panic
8.2	exo-driver-sdk crate (GDI trait + types)	Syscalls 8.1	Crate partagée par tous les drivers DeviceMatch, ProbeResult, IrqResult	cargo build --no-default-features sur la crate seule
8.3	device_server — PCI scanner + static PatchDB	exo-driver-sdk	device_server démarre, scanne PCI, liste les devices en log	QEMU : 4 devices Virtio détectés
8.4	virtio-drivers HAL ExoOS + virtio-blk driver	device_server 8.3	Un fichier lisible sur /dev/vda via SYS_EXOFS_OPEN_BY_PATH	dd if=/dev/vda bs=512 count=1 fonctionne
8.5	virtio-net driver	virtio-blk 8.4	Ping depuis ExoOS VM	ping 8.8.8.8 = TTL réponse
8.6	PatchDB dynamique depuis ExoFS	ExoFS monté (Phase 4)	Patch chargé au runtime depuis ExoFS Hash Blake3 vérifié	Carte Ethernet Intel I225 détectée avec patch appliqué
8.7	USB HID driver (clavier/souris)	XHCI driver + GDI	Clavier USB physique fonctionnel	Frappe au clavier → caractères écran
8.8	Linux Shim — alloc + mmio + irq	exo-driver-sdk	linux_shim.so compilé driver Linux basique chargeable	e1000e driver Linux recompilé fonctionne en Ring 1 ExoOS

📋  Chaque étape est testable indépendamment. Ne pas passer à l'étape N+1 sans que les tests de l'étape N soient verts. Un driver qui 'marche à peu près' en Phase 8 causera des DRV-SILENT non détectés qui resurgiront en Phase 9.
