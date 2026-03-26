ExoOS
Guide d'Implémentation Complet — v2
Toutes phases · Code Rust no_std réel · 30 erreurs intégrées dans leur phase + référence rapide
Ce document est la source de vérité pour toute IA codeur implémentant ExoOS.
Mars 2026 · ExoOS Project

0. Contexte projet — Lire avant la première ligne de code

0.1 Architecture générale
ExoOS est un microkernel from-scratch Rust, x86_64, bare-metal. Pas de dérivation d'un OS existant. Trois rings :
•	Ring 0 = Kernel. Deux instances coexistent : Kernel A (actif, gère les services) et Kernel B (ExoPhoenix, sentinelle permanente qui surveille A et le reconstruit s'il est compromis).
•	Ring 1 = Serveurs système. Drivers, filesystem server, IPC router. Isolés par capabilities.
•	Ring 3 = Applications utilisateur. Jamais accès direct au hardware.

0.2 Décisions locked — jamais à remettre en cause
Décision	Raison
TCB = 256 bytes (4 cache lines)	18 GPR×8=144 + fs_base+gs_base (TLS obligatoire userspace) + pkrs + metadata + padding = 256. 128 bytes était impossible.
SSR à 0x1000000, 64 Ko	Adresse physique fixe partagée A↔B. Réservée dans e820 AVANT tout init du buddy allocator.
MAX_CORES = 64	Slots SSR de 64 bytes par core. APIC IDs épars → table apic_to_slot[256]. Jamais apic_id*64 direct.
ExoFS = content-addressed, syscalls 500-520	Hash Blake3 = identité du fichier. Non-POSIX. ExoForge recharge depuis hash garanti. Immuable par design.
Pas d'hyperviseur (pas VT-x/SVM)	ExoPhoenix fait ce qu'un hyperviseur ferait, bare-metal pur. VMXOFF si VT-x actif par firmware.
TLA+/Spin bloque Phase 3.6	handoff.rs ne se code PAS avant preuve formelle de l'automate PhoenixState. Ce n'est pas optionnel.
Boot = GRUB + Multiboot2	Chemin dev/CI actif. ExoBoot UEFI (exo-boot/) existe mais GRUB reste le chemin pour QEMU.

0.3 Invariants permanents — si le code les viole, le système est non-sécurisé
⚠ Règles absolues. Aucune exception. Aucune 'optimisation' qui les contourne.

•	La SSR (0x1000000) n'est JAMAIS allouée par le buddy. Réservation e820 précède tout.
•	Les handlers 0xF1/0xF2/0xF3 sont LOCK-FREE absolus. Zéro spinlock. Uniquement atomic stores + rdmsr/wrmsr.
•	B n'exécute JAMAIS de code Ring 3 en état Normal.
•	Le SIPI vers A est la DERNIÈRE instruction du Stage 0. Toutes protections actives avant.
•	SSR champs critiques = Release/Acquire uniquement. Jamais Relaxed sur HANDOFF_FLAG, FREEZE_ACK, LIVENESS_NONCE.
•	apic_to_slot[apic_id] = slot_index. Jamais apic_id*64 directement dans les offsets SSR.

 
Phase 0	Déblocage Workspace	CORRIGER · 2-4 heures

0.1 — Fix libs/exo-alloc
▸ Contexte
libs/Cargo.toml déclare exo-alloc comme membre workspace mais le répertoire est absent. Cargo refuse de compiler quoi que ce soit.
📄 libs/Cargo.toml — retirer la ligne membre OU créer le stub
# Option A (recommandée si obsolète) : retirer la ligne dans libs/Cargo.toml
# Option B : créer le stub minimal
mkdir -p libs/exo-alloc/src
# libs/exo-alloc/Cargo.toml
[package]
name = "exo-alloc"
version = "0.1.0"
edition = "2021"
[lib]
# libs/exo-alloc/src/lib.rs
#![no_std]
// Stub — Phase 4

✓ Test : cd libs && cargo check — doit passer sans erreur de workspace.

0.2 — Fix toolchain nightly
▸ Contexte
'nightly-x86_64-unknown-linux-musl' est un format invalide. Le triple appartient dans targets, pas dans channel.
📄 kernel/rust-toolchain.toml
[toolchain]
channel = "nightly"
targets = ["x86_64-unknown-none"]
components = ["rust-src", "llvm-tools-preview"]

✓ Test : cd kernel && cargo check — l'erreur toolchain disparaît.

G5	Erreur intégrée : buddy is_empty(NULL) — crash heap en production

▸ Contexte
FreeNode::is_empty() déréférence list_head sans guard null. Une zone mémoire non-initialisée peut passer un pointeur null. En Ring 0, déréférencement null = Triple Fault = reboot silencieux sans log.
📄 kernel/src/memory/physical/allocator/buddy.rs
fn is_empty(list_head: *const FreeNode) -> bool {
    if list_head.is_null() { return true; }  // ← AJOUT OBLIGATOIRE
    // ... logique existante inchangée
}

✗ ERREUR — Ne pas réécrire la fonction. Ajouter uniquement le guard null en tête.
✓ Test : boot QEMU → atteint halt_cpu() sans Triple Fault.

 
Phase 1	TCB 256 bytes + SSR figée + e820	INTÉGRER + CRÉER · 2-3 jours

Ces structures sont partagées entre Kernel A et Kernel B. Figées AVANT tout le reste. Un changement post-code casse tout.

1.1 — Révision TCB vers 256 bytes
▸ Pourquoi 256 et pas 128 — calcul réel
•	18 GPR (rax..r15, rip, rsp, rflags, cs/ss, cr2) × 8 = 144 bytes pour cpu_ctx seul.
•	fs_base (8 bytes) : MSR 0xC0000100 — valeur TLS par thread. Obligatoire pour pthreads, Rust std. Sans ça, tout programme userspace multi-threadé a des données corrompues.
•	gs_base (8 bytes) : MSR 0xC0000101 — valeur GS userspace. SWAPGS au syscall entry = valeur par thread obligatoire.
•	pkrs (4 bytes) : Intel Protection Key Rights Supervisor. 0 sur AMD. Conditionnel CPUID. 2 instructions au context switch si PKS disponible, zéro coût sur AMD.
•	metadata (capability_token, kstack_ptr, tid, sched_state) = 32 bytes.
•	Total : 32 + 8 + 8 + 4 + 12 padding + 144 + 48 padding = 256 bytes (4 cache lines).

▸ Pourquoi cpu_ctx doit être inline
Si cpu_ctx est un pointeur vers la kstack : context switch = déréférencement TCB + déréférencement pointeur + accès registres = 3 accès mémoire potentiellement sur 3 cache lines différentes. Inline = 1 accès contigu. Sur le chemin le plus chaud du kernel.

📄 kernel/src/scheduler/core/task.rs — fichier existant à modifier (165 références)

#[repr(C, align(64))]  // align(64) OBLIGATOIRE : alignement cache line SMP
pub struct ThreadControlBlock {
    // Cache Line 1 [0..63] : données chaudes scheduler
    pub capability_token: u64,    // [0]  Token capacité ExoOS
    pub kstack_ptr:       u64,    // [8]  RSP Ring 0
    pub tid:              u64,    // [16] Thread ID
    pub sched_state:      u64,    // [24] RUNNING/BLOCKED/ZOMBIE
    pub fs_base:          u64,    // [32] MSR 0xC0000100 TLS userspace
    pub gs_base:          u64,    // [40] MSR 0xC0000101 GS userspace
    pub pkrs:             u32,    // [48] Intel PKS (0 sur AMD)
    _pad1:                [u8;12],// [52] → 64 bytes
    // Cache Lines 2-4 [64..255] : CPU context inline
    pub cpu_ctx:          CpuContext,
}

#[repr(C)]
pub struct CpuContext {
    pub gpr:     [u64; 16], // rax,rbx,rcx,rdx,rsi,rdi,rbp,rsp,r8..r15
    pub rip:     u64,
    pub rsp_usr: u64,       // RSP userspace
    pub rflags:  u64,
    pub cs_ss:   u64,       // cs<<32|ss
    pub cr2:     u64,       // page fault addr
    _pad:        [u8; 24],  // → 192 bytes
}

// Tests obligatoires dans tests/layout.rs
const _: () = assert!(core::mem::size_of::<ThreadControlBlock>() == 256);
const _: () = assert!(core::mem::align_of::<ThreadControlBlock>() == 64);

⚠ 165 références au TCB dans le codebase. Ne pas modifier les offsets des champs existants. Ajouter fs_base/gs_base/pkrs À LA FIN de la cache line 1, avant le padding.

S6	Erreur intégrée : PKRS non sauvegardé au context switch

// Dans la fonction switch_context — conditionnel CPUID
if B_FEATURES.pks_available {
    // Sauvegarder PKRS du thread sortant
    outgoing_tcb.pkrs = rdpkrs();
    // Restaurer PKRS du thread entrant
    wrpkrs(incoming_tcb.pkrs);
}
// Sur AMD : B_FEATURES.pks_available == false → zéro coût

1.2 — SSR Layout
📄 kernel/src/exophoenix/ssr.rs — fichier à CRÉER

ℹ Ce fichier doit être compilé IDENTIQUEMENT dans Kernel A et Kernel B. Si les offsets divergent, les atomic stores de A tombent dans les champs de B. Corruption silencieuse.

//! SSR — Shared State Region
//! Adresse physique fixe : 0x1000000 | Taille : 64 Ko | MAX_CORES : 64
use core::sync::atomic::{AtomicU64, Ordering};

pub const SSR_BASE:           u64   = 0x100_0000;
pub const SSR_SIZE:           usize = 0x10000;
pub const MAX_CORES:          usize = 64;

// Offsets — ne jamais accéder à ces adresses sans les helpers ci-dessous
pub const SSR_HANDOFF_FLAG:   usize = 0x0000; // AtomicU64
  // États : 0=NORMAL 1=FREEZE_REQ 2=FREEZE_ACK_ALL 3=B_ACTIVE
pub const SSR_LIVENESS_NONCE: usize = 0x0008; // AtomicU64
  // B écrit nonce (RDRAND), A copie, B vérifie par PULL
pub const SSR_SEQLOCK:        usize = 0x0010; // AtomicU64
pub const SSR_CMD_B2A:        usize = 0x0040; // 64 bytes
pub const SSR_FREEZE_ACK:     usize = 0x0080; // 64 bytes × MAX_CORES
pub const SSR_PMC_SNAPSHOT:   usize = 0x1080; // 64 bytes × MAX_CORES
pub const SSR_LOG_AUDIT:      usize = 0x8000; // 16 Ko RO pour A
pub const SSR_METRICS_PUSH:   usize = 0xC000; // 16 Ko

pub const FREEZE_ACK_DONE:    u64   = 0xACED_0001;
pub const TLB_ACK_DONE:       u64   = 0xACED_0002;

// TOUJOURS utiliser ces helpers — jamais apic_id*64 directement (erreur S10)
#[inline(always)]
pub fn freeze_ack_offset(slot_index: usize) -> usize {
    SSR_FREEZE_ACK + slot_index * 64
}
#[inline(always)]
pub fn pmc_snapshot_offset(slot_index: usize) -> usize {
    SSR_PMC_SNAPSHOT + slot_index * 64
}
pub unsafe fn ssr_atomic(offset: usize) -> &'static AtomicU64 {
    &*((SSR_BASE as usize + offset) as *const AtomicU64)
}

S3	Erreur intégrée : SSR layout divergent A/B

Règle : tous les accès SSR dans Kernel A ET Kernel B passent par ce même fichier ssr.rs. Si le fichier est copié, c'est une erreur. Il doit être un module partagé.

S10	Erreur intégrée : APIC ID sparse → overflow SSR

Les APIC IDs x86_64 modernes peuvent être épars : 0,2,4 (hyperthreading), 0,32,64 (NUMA). apic_id*64 avec un APIC ID de 128 = écriture 8192 bytes hors de la zone PMC. La table apic_to_slot est construite au Stage 0 (étape 6) depuis la MADT réelle.
// Table construite au Stage 0 — immuable après
static APIC_TO_SLOT: [u8; 256] = [0xFF; 256]; // 0xFF = non assigné
// Dans les handlers : let slot = APIC_TO_SLOT[read_apic_id() as usize];
//                     assert!(slot != 0xFF);

1.3 — Réservation e820
📄 kernel/src/arch/x86_64/boot/memory_map.rs — à modifier

S7	Erreur intégrée : e820 sans réservation SSR — buddy alloue la SSR de A

Si cette réservation est absente ou placée APRÈS l'init du buddy, le buddy peut allouer la région SSR à un processus de A. A écrase la SSR, B lit des données corrompues.
// AVANT tout init du buddy allocator
memory_map.mark_reserved(
    PhysAddr(ssr::SSR_BASE),
    ssr::SSR_SIZE,
    MemoryType::ExoPhoenixSSR,
);
✓ Test : après boot, vérifier que buddy ne retourne jamais d'adresse dans [0x1000000..0x10FFFF].

 
Phase 2	verify_cap + TOCTOU dans les handlers ExoFS	CORRIGER · 3-5 jours

S2	Erreur intégrée : verify_cap absent dans les handlers ExoFS

▸ Contexte
Les 21 handlers ExoFS compilent et fonctionnent. Mais ils traitent les requêtes sans vérifier que le processus appelant a les droits requis. N'importe quel processus Ring 3 peut appeler sys_exofs_object_read() et lire n'importe quel objet du filesystem. Aucune erreur à la compilation.
# Diagnostic : si ce grep retourne 0 lignes, la faille est confirmée
grep -r 'verify_cap' kernel/src/fs/exofs/syscall/

G7	Erreur intégrée : TOCTOU sur verify_cap — params userspace modifiables après vérif

▸ Le TOCTOU expliqué
Un thread malveillant peut modifier la mémoire userspace pointée par ctx.arg0 APRÈS que verify_cap() a validé les paramètres, mais AVANT que le handler utilise ces paramètres. L'attaquant fait valider des paramètres légitimes, puis substitue des paramètres malveillants.
La solution : copier les paramètres en kernel space AVANT verify_cap(). La copie est immuable — Ring 3 ne peut plus la modifier.

// Pattern obligatoire pour CHAQUE handler ExoFS
pub fn sys_exofs_object_read(ctx: &SyscallContext) -> SyscallResult {
    // ÉTAPE 1 — Copier depuis userspace AVANT verify_cap
    //   copy_from_user vérifie que l'adresse appartient à Ring 3
    //   et crée une copie en kernel space immuable
    let params = copy_from_user::<ExoFsReadParams>(ctx.arg0 as usize)?;

    // ÉTAPE 2 — verify_cap sur la copie kernel
    verify_cap(ctx.process, CapabilityType::ExoFsRead)?;

    // ÉTAPE 3 — Utiliser params (copie kernel, Ring 3 ne peut plus la modifier)
    let hash = params.object_hash;
    // ...
}

📌 21 handlers à modifier. Commencer par les handlers destructifs (write, delete, epoch_commit) puis les read-only. Chaque handler a son CapabilityType correspondant.
✓ Test : processus Ring 3 sans capability → EPERM. Avec capability → succès.

 
Phase 3	ExoPhoenix — Kernel B complet	CRÉER · 8-12 semaines

ℹ ExoPhoenix n'existe pas encore dans le code. Lire entièrement cette section avant d'écrire la première ligne. Chaque module dépend des précédents.

3.0 — Architecture ExoPhoenix
Rôle de chaque composant
Composant	Rôle exact
stage0.rs	Boot de B en 14 étapes. Configure IOMMU, IST, APIC, calibration timer, FACS RO. Envoie SIPI vers A en DERNIER.
sentinel.rs	Boucle infinie sur Core 0. Introspection de A toutes les T_detection. Détecte les compromissions. Met à jour PhoenixState.
handoff.rs	Orchestration du gel de A (Phase 1 coopérative + Phase 2 forcée). BLOQUÉ jusqu'à validation TLA+.
forge.rs	Reconstruction de A depuis image propre ExoFS. Parser ELF safe, vérification Merkle, reset Ring 1.
isolate.rs	Cage mémoire de A pendant l'isolation. IOMMU + pages !PRESENT + IDT override.

Les 3 vecteurs IPI réservés ExoPhoenix
Vecteur	Nom	Comportement du core A qui reçoit l'IPI
0xF1	Freeze	cli + CAS atomic ACK dans SSR[freeze_ack_offset(slot)] + boucle pause. Core gelé jusqu'à réveil.
0xF2	PMC Snapshot	rdmsr(EVTSEL0..3, CTR0..3) + write_volatile dans SSR[pmc_snapshot_offset(slot)]. Retourne immédiatement.
0xF3	TLB Flush	mov cr3,cr3 (invalide tout le TLB local) + ACK dans SSR. Retourne normalement. Obligatoire après modif PTEs de A par B.

⚠ 0xF1, 0xF2, 0xF3 sont RÉSERVÉS. Ajouter un assert dans l'allocateur IRQ : assert!(vector != 0xF1 && vector != 0xF2 && vector != 0xF3)

3.1 — IDT : réserver les vecteurs + configurer IST
G-N1	Erreur intégrée : IST non configuré — Triple Fault si RSP de A est corrompu

▸ Pourquoi l'IST est BLOQUANT pour tout le reste
A peut corrompre son propre RSP (pointeur de pile) avant d'être gelé. Quand B lui envoie l'IPI 0xF1, le processeur tente de créer le stack frame de l'interrupt sur la pile corrompue. Résultat : Double Fault. Puis Triple Fault. Reboot matériel sans log. Aucune récupération possible.
L'Interrupt Stack Table (IST) est le mécanisme x86_64 qui force l'utilisation d'une pile DÉDIÉE et VALIDE pour un handler, indépendamment de l'état du RSP du contexte interrompu. Elle est configurée dans le TSS.

📄 kernel/src/arch/x86_64/interrupts/tss.rs — à modifier
📄 kernel/src/arch/x86_64/interrupts/idt.rs — à modifier

// Étape 3 du Stage 0 — avant IDT
pub fn init_b_tss(early_pool: &mut BumpAllocator) -> TaskStateSegment {
    // 3 piles IST distinctes — chacune avec guard page
    let ist0 = alloc_guarded_stack(early_pool, 0x4000); // 0xF1/0xF2/0xF3
    let ist1 = alloc_guarded_stack(early_pool, 0x4000); // #PF
    let ist2 = alloc_guarded_stack(early_pool, 0x4000); // NMI
    let mut tss = TaskStateSegment::new();
    tss.interrupt_stack_table[0] = VirtAddr::new(ist0);
    tss.interrupt_stack_table[1] = VirtAddr::new(ist1);
    tss.interrupt_stack_table[2] = VirtAddr::new(ist2);
    tss
}

// Étape 4 — IDT avec IST index pour les handlers critiques
idt[0xF1].set_handler_fn(handler_freeze).set_stack_index(0);
idt[0xF2].set_handler_fn(handler_pmc).set_stack_index(0);
idt[0xF3].set_handler_fn(handler_tlb).set_stack_index(0);
idt[PageFault].set_handler_fn(handler_pf_b).set_stack_index(1);
idt[Nmi].set_handler_fn(handler_nmi_b).set_stack_index(2);

✗ ERREUR — Oublier set_stack_index() = le handler utilise le RSP du contexte interrompu. Si ce RSP est corrompu = Triple Fault garanti.

3.2 — Module ExoPhoenix : structure et automate
📄 kernel/src/exophoenix/mod.rs — à CRÉER

pub mod ssr;      // Phase 1.2
pub mod stage0;   // Phase 3.3
pub mod sentinel; // Phase 3.5
pub mod handoff;  // Phase 3.6 — BLOQUÉ TLA+
pub mod forge;    // Phase 3.7
pub mod isolate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PhoenixState {
    BootStage0   = 0,
    Normal       = 1,
    Threat       = 2,
    IsolationSoft= 3,  // Freeze coopératif Phase 1
    IsolationHard= 4,  // Kill forcé Phase 2
    Certif       = 5,
    Restore      = 6,
    Degraded     = 7,
    Emergency    = 8,
}

pub static PHOENIX_STATE: AtomicU8 =
    AtomicU8::new(PhoenixState::BootStage0 as u8);

3.3 — Stage 0 : les 14 étapes
G1	Erreur intégrée : SIPI avant Stage 0 complet
S-N3	Erreur intégrée : x2APIC non détecté — APIC MMIO silencieux
S-N4	Erreur intégrée : CPUID probe absent
G-N2	Erreur intégrée : APIC Timer non calibré — timeouts faux
G-N3	Erreur intégrée : VMXOFF absent si VT-x actif

▸ L'ordre est STRICT. Toute déviation = bug de sécurité.

pub fn stage0_init() -> ! {
    // ─ 1. Page tables de B ─────────────────────────────────────
    let cr3 = setup_b_page_tables();
    unsafe { asm!("mov cr3, {}", in(reg) cr3) };

    // ─ 0.5. CPUID feature probe (S-N4) ─────────────────────────
    // AVANT tout accès hardware — détecte : x2apic, pks, invpcid,
    // invariant_tsc, pmc (leaf 0xA), hpet, pcid, vmx_active
    B_FEATURES.init();

    // ─ 2. Stack de B + guard page ───────────────────────────────
    setup_b_stack_with_guard_page();

    // ─ 3. IST dans TSS (G-N1) ──────────────────────────────────
    let tss = init_b_tss(&mut EARLY_POOL);
    load_tss(tss);

    // ─ 4. IDT avec handlers stubs ───────────────────────────────
    setup_b_idt_with_ist();

    // ─ 5. Parse ACPI (MADT, FADT, FACS) ────────────────────────
    let acpi = parse_acpi();

    // ─ 5.5. Énumération PCI ─────────────────────────────────────
    let devices = enumerate_pci();
    let pool_r3_sz = calc_pool_r3_size(&devices); // formule V4-M2

    // ─ 6. Table apic_to_slot[256] ───────────────────────────────
    build_apic_to_slot(&acpi.madt);

    // ─ 7. Calibration APIC Timer (G-N2) ─────────────────────────
    // Sans calibration, 100µs/200µs/T_detection sont des fictions
    TICKS_PER_US.store(calibrate_apic_via_pit(), Ordering::Relaxed);

    // ─ 8. Init APIC + VMXOFF si VT-x actif (S-N3, G-N3) ─────────
    if B_FEATURES.vmx_active {
        unsafe { asm!("vmxoff") };
        write_cr4(read_cr4() & !CR4_VMXE);
    }
    init_local_apic_dispatch(); // xAPIC ou x2APIC selon B_FEATURES

    // ─ 9. IOMMU + PCIe ACS + IOTLB flush (S-N1) ─────────────────
    setup_iommu(&acpi, &devices);   // deny-by-default
    enable_pcie_acs(&devices);      // anti DMA peer-to-peer

    // ─ 10. FACS RO + hash MADT ──────────────────────────────────
    mark_facs_ro_in_a_pts(&acpi.facs);
    MADT_HASH.store(hash_madt(&acpi.madt), Ordering::Relaxed);

    // ─ 11. Pool R3 ──────────────────────────────────────────────
    POOL_R3.init(R3_BASE, pool_r3_sz);

    // ─ 12. Watchdog timer ────────────────────────────────────────
    arm_watchdog_ticks(WATCHDOG_MS * ticks_per_ms());

    // ─ 13. SIPI vers A — DERNIÈRE INSTRUCTION (G1) ──────────────
    PHOENIX_STATE.store(PhoenixState::Normal as u8, Ordering::Release);
    send_sipi_once(CORE_A_SLOT, A_ENTRY_VECTOR).expect("SIPI échoué");
    sentinel::run_forever()
}

▸ APIC : dispatch x2APIC vs xAPIC (S-N3)
Sur hardware moderne (Intel Skylake+, AMD EPYC/Ryzen serveur), le firmware peut activer x2APIC. En x2APIC, les accès MMIO à 0xFEE00000 sont silencieusement ignorés. Seuls les MSRs fonctionnent.
fn apic_write(reg: ApicReg, val: u32) {
    match B_FEATURES.apic_mode {
        ApicMode::XApic  => mmio_write(APIC_BASE + reg.offset(), val),
        ApicMode::X2Apic => wrmsr(reg.msr(), val as u64),
    }
}

▸ Calibration APIC Timer (G-N2)
L'APIC Timer compte en ticks bruts — la fréquence varie par CPU et P-state. Sans calibration, timeout(100µs) ne signifie rien.
fn calibrate_apic_via_pit() -> u64 {
    pit_set_oneshot(CHANNEL_2, 10_000); // 10ms
    apic_write(TIMER_INIT, u32::MAX);
    while pit_ch2_running() {}
    let remaining = apic_read(TIMER_CURRENT);
    (u32::MAX - remaining) as u64 / 10_000 // ticks par µs
}

3.4 — Handlers IPI 0xF1, 0xF2, 0xF3
S1	Erreur intégrée : Spinlock dans handler IPI
S11	Erreur intégrée : #GP dans 0xF2 si PMC non supporté
G6	Erreur intégrée : CLI ne bloque PAS les NMI — limite documentée

▸ Règle absolue lock-free
Ces handlers peuvent préempter n'importe quel code de A qui détient un spinlock. Si le handler tente d'acquérir un lock → deadlock. Le core ne répond plus. B ne reçoit jamais l'ACK. Tout le handoff est bloqué.

// Handler 0xF1 — Freeze coopératif
unsafe extern "x86-interrupt"
fn handler_freeze(_f: InterruptStackFrame) {
    asm!("cli"); // bloque IRQs maskables — NMI reste non-bloquable (G6)
    let slot = APIC_TO_SLOT[read_lapic_id() as usize] as usize;
    assert!(slot != 0xFF);
    // Release : B lit avec Acquire (S9)
    ssr_atomic(freeze_ack_offset(slot))
        .store(FREEZE_ACK_DONE, Ordering::Release);
    asm!("sfence");
    loop { asm!("pause"); } // gelé ici jusqu'à réveil
}

// Handler 0xF3 — TLB flush (S8 : INVPCID ne fait que le core local)
unsafe extern "x86-interrupt"
fn handler_tlb(_f: InterruptStackFrame) {
    asm!("cli");
    let cr3: u64;
    asm!("mov {}, cr3", out(reg) cr3); // lire CR3
    asm!("mov cr3, {}", in(reg) cr3);  // invalide tout le TLB
    let slot = APIC_TO_SLOT[read_lapic_id() as usize] as usize;
    ssr_atomic(freeze_ack_offset(slot))
        .store(TLB_ACK_DONE, Ordering::Release);
    // retour normal — pas de boucle hlt
}

// Handler 0xF2 — PMC snapshot (heuristique faible)
unsafe extern "x86-interrupt"
fn handler_pmc(_f: InterruptStackFrame) {
    // S11 : #GP si PMC non supporté — vérifier AVANT rdmsr
    if !B_FEATURES.pmc_available.load(Ordering::Relaxed) { return; }
    let slot = APIC_TO_SLOT[read_lapic_id() as usize] as usize;
    let base = SSR_BASE as usize + pmc_snapshot_offset(slot);
    write_volatile(base as *mut u64,       rdmsr(IA32_PERF_EVTSEL0));
    write_volatile((base+8) as *mut u64,  rdmsr(IA32_PERF_CTR0));
    // ... EVTSEL1..3, CTR1..3
}

📌 G6 — CLI ne bloque pas les NMI par définition x86. Si une NMI firmware (thermique, ECC) arrive pendant le freeze, le core l'exécute dans son IST stack (IST[2]) puis revient dans la boucle pause. Si le NMI handler de A est propre (ExoForge le garantit), c'est acceptable. NMI comme vecteur d'attaque délibéré = attaque physique, hors scope ExoPhoenix.

3.5 — sentinel.rs
S-N2	Erreur intégrée : SMI firmware — faux positif T_detection
S4	Erreur intégrée : PT walker récursif — stack overflow B
S5	Erreur intégrée : PMC snapshot comme source de vérité
S8	Erreur intégrée : INVPCID local seulement — TLB shootdown via IPI 0xF3

pub fn run_forever() -> ! {
    loop {
        let t0 = read_apic_timer();
        let score = run_introspection_cycle();
        let elapsed_us = apic_ticks_to_us(t0 - read_apic_timer());

        // S-N2 : SMI suspend tous les cores 1-100ms
        // Un cycle 3× trop long = SMI probable, pas une attaque
        if elapsed_us > T_DETECTION_US * 3 {
            SMI_COUNTER.fetch_add(1, Ordering::Relaxed);
            continue; // skiper — pas de fausse alerte
        }

        if score >= THREAT_THRESHOLD {
            PHOENIX_STATE.store(PhoenixState::Threat as u8, Ordering::Release);
            // handoff::begin_isolation(); — Phase 3.6 bloquée TLA+
        }
        wait_ticks(T_DETECTION_US * TICKS_PER_US.load(Ordering::Relaxed));
    }
}

▸ PT Walker itératif (S4 : pas de récursivité)
fn walk_a_page_tables() -> u32 {
    let mut score = 0u32;
    let mut pf_count = 0u32;
    let max_steps = A_PAGES_KNOWN * 4;
    let mut steps = 0usize;
    // PML4 → PDPT → PD → PT : 4 niveaux max, boucles for
    for pml4i in 0..512 {
        steps += 1; if steps > max_steps { break; }
        let pdpt_pa = read_pte_safe(a_cr3, pml4i, &mut pf_count);
        if pf_count > PF_HIGH  { score += SCORE_PF_FLOOD; break; }
        if pf_count > PF_LOW   { /* skip sous-arbre */ continue; }
        if pdpt_pa & PRESENT == 0 { continue; }
        // N2 : bounds check PA (3 comparaisons, zéro footprint)
        if is_b_region(pdpt_pa & PHYS_MASK) { score += SCORE_PA_REMAP; }
        // ... niveaux PD et PT
    }
    score
}
// S8 : après modif PTEs de A par B → IPI 0xF3 broadcast + wait ACK TLB
fn apply_pte_protection_and_flush(addr: VirtAddr) {
    b_modify_pte(addr, PTE_RO); // B modifie la PTE de A
    send_ipi_broadcast(0xF3);   // invalider TLB de tous les cores A
    wait_tlb_acks();            // attendre ACK_TLB dans SSR
}

📌 S5 — Le PMC snapshot est UNE SOURCE PARMI D'AUTRES. A peut avoir hooké le handler 0xF2 et forger les valeurs. Un snapshot 'clean' ne peut jamais abaisser le score ni invalider une alerte basée sur le walker ou le liveness nonce.

3.6 — handoff.rs
⛔ BLOQUÉ — Phase 3.6 est bloquée jusqu'à la validation TLA+/Spin de l'automate PhoenixState v5. Ne pas coder handoff.rs avant que la preuve formelle ne confirme l'absence de deadlock.

G2	Erreur intégrée : INIT IPI sans masquage MSI/MSI-X préalable
G4	Erreur intégrée : Handoff sans drain IOMMU simultané
S-N1	Erreur intégrée : IOMMU IOTLB non flushed après hard revoke
G8	Erreur intégrée : Double SIPI — bitset SIPI_SENT

▸ Séquence Phase 1 — Freeze coopératif (après TLA+)
pub fn begin_isolation_soft() {
    // G4 : IPI 0xF1 ET soft revoke IOMMU démarrent SIMULTANÉMENT
    send_ipi_broadcast_0xf1();
    iommu_soft_revoke_all(); // désactive nouvelles soumissions DMA
    // Attendre les deux conditions
    let deadline = now_us() + 100;
    while !all_cores_acked() || !iommu_drain_done() {
        if now_us() > deadline { escalate_to_hard(); return; }
    }
    // Hard revoke + IOTLB flush (S-N1 : tables + QI + Completion Wait)
    iommu_hard_revoke_with_iotlb_flush();
    swap_to_b_cr3(); // B prend le contrôle
}

▸ Séquence Phase 2 — Kill forcé
pub fn begin_isolation_hard() {
    // G2 : masquer MSI/MSI-X AVANT INIT IPI
    mask_all_msi_msix();
    // INIT IPI — reset matériel du core
    send_init_ipi_to_resistant_cores();
    // S-N1 : hard revoke sans drain (Ring 1 gelé)
    iommu_hard_revoke_with_iotlb_flush();
    scan_and_release_spinlocks(); // nettoyage locks détenus par cores tués
}

3.7 — forge.rs
G3	Erreur intégrée : Reload driver sans device reset
G9	Erreur intégrée : FACS RO non re-vérifié post-forge

ExoForge reconstruit A depuis une image propre stockée dans ExoFS (content-addressed = hash garanti). Parser ELF 100% safe Rust, arena allocator borné.
pub fn reconstruct_kernel_a() -> Result<(), ForgeError> {
    // 1. Charger l'image de A depuis ExoFS (hash vérifié)
    let image = exofs_load_by_hash(A_IMAGE_HASH)?;
    // 2. Parser ELF avec crate 'nom' no_std (safe Rust uniquement)
    let elf = parse_elf_safe(&image)?;
    // 3. Vérification Merkle .text + .rodata
    verify_merkle_tree(&elf, &TRUSTED_MERKLE_ROOT)?;
    // 4. Recharger .data depuis image propre, zéroter .bss
    reload_data_sections(&elf);
    zero_bss_section(&elf);
    // 5. Reset Ring 1 (G3 : device reset AVANT reload binaire)
    for driver in ring1_drivers() {
        pci_function_level_reset(driver.pci_dev);  // FLR hardware
        drain_dma_queues(driver);                   // vider queues DMA
        iommu_iotlb_flush();                        // invalider IOTLB
        reload_driver_binary(driver);               // recharger depuis ExoFS
    }
    // 6. Checklist post-reconstruction OBLIGATOIRE (G9)
    mark_facs_ro_in_a_pts(&ACPI_FACS); // re-marquer RO
    verify_madt_hash(&MADT_HASH)?;     // hash MADT inchangé
    tlb_shootdown_all_a_cores();        // invalider TLBs
    verify_idt_has_exophoenix_vectors();// 0xF1/0xF2/0xF3 présents
    Ok(())
}

 
Phase 4	Servers Ring 1 — 5 stubs à compléter	COMPLÉTER · 4-6 semaines

4 servers sont fonctionnels (crypto_server, init_server, ipc_router, vfs_server). 5 sont des stubs loop{}. Ordre de priorité : memory_server en premier car les autres en dépendent.

4.1 — Ordre de priorité
Server	Priorité	Dépendances et rôle
memory_server	1 — CRITIQUE	Gère les allocations mémoire userspace. Dépend : buddy corrigé (G5) + capabilities (Phase 2). Bloque tout userspace.
device_server	2 — HAUTE	Cycle de vie des drivers Ring 1. Dépend : memory_server. ExoPhoenix surveille ses pages (forge.rs G3).
scheduler_server	3 — HAUTE	Scheduling Ring 1/3. Dépend : TCB révisé (Phase 1). Interface avec kernel scheduler.
network_server	4 — MOYENNE	Dépend : device_server + drivers réseau (Phase 5). Blocker pour téléchargements → ExoFS ingestion.
exo_shield	5 — ExoPhoenix	Interface ExoPhoenix↔Ring 1. À implémenter APRÈS Phase 3 complète. N'existe pas encore — CRÉER.

4.2 — Pattern base de chaque server
Chaque server Ring 1 suit le même cycle de vie : enregistrement auprès d'init_server, vérification capability, boucle IPC.
// servers/memory_server/src/main.rs
#![no_std]
use exo_ipc::{ipc_receive, ipc_reply};
use exo_security::verify_cap;

fn main() {
    // 1. S'enregistrer — obtenir sa propre capability
    let cap = init_server::register(ServerType::Memory)
        .expect("enregistrement échoué");
    cap.verify(CapabilityType::MemoryServer)
        .expect("capability invalide");
    // 2. Boucle IPC
    loop {
        let msg = ipc_receive();
        let resp = match msg.type_ {
            MemoryRequest::Alloc(size)      => handle_alloc(size),
            MemoryRequest::Free(addr)       => handle_free(addr),
            MemoryRequest::MapShared(range) => handle_map_shared(range),
            _ => Err(ExoError::ENOSYS),
        };
        ipc_reply(msg.sender, resp);
    }
}

📌 Chaque server doit vérifier sa propre capability au démarrage. Un server sans capability valide ne doit pas démarrer — paniquer avec un message clair.

 
Phase 5	Drivers Virtio — priorité block, net, console	CRÉER · 3-5 semaines

17 main.rs vides dans drivers/. Ordre : virtio-block en premier car requis pour ExoFS sur disque. Décision locked : Virtio avant drivers hardware réels.

5.1 — Ordre de priorité
Driver	Priorité	Rôle
virtio-block	1 — CRITIQUE	Stockage QEMU. Sans ça, ExoFS existe uniquement en RAM. Requis pour forge.rs (recharge A depuis disque).
virtio-net	2 — HAUTE	Réseau QEMU. Requis pour tester téléchargement → ExoFS ingestion (fichier externe entrant).
virtio-console	3 — HAUTE	Console QEMU. Debug interactif et interaction utilisateur.
drivers/fs/ (existant)	4 — VALIDER	Seul driver non-vide. Lire, comprendre, connecter à ExoFS.
drivers hardware réels	5 — PLUS TARD	AHCI, NVMe, USB, etc. Après validation complète sur QEMU.

5.2 — Architecture d'un driver Virtio
Chaque driver Virtio tourne en Ring 1, s'enregistre auprès de device_server, et communique via des Virtio queues (split ring ou packed ring).
// drivers/virtio-block/src/main.rs
fn main() {
    // 1. Trouver le device Virtio Block via device_server
    let dev = device_server::find(VirtioDeviceType::Block)
        .expect("virtio-block non trouvé");
    // 2. Initialiser les queues Virtio
    let queue = VirtioQueue::new(dev.bar0, QUEUE_SIZE);
    queue.negotiate_features(VIRTIO_BLK_F_RO | VIRTIO_BLK_F_SIZE_MAX);
    // 3. Enregistrer auprès de ExoFS comme backend de stockage
    exofs_register_storage_backend(BlockDevice { queue });
    // 4. Boucle de traitement
    loop {
        let req = block_request_queue.pop();
        handle_block_request(req, &mut queue);
    }
}

📌 Les drivers Virtio sont surveillés par ExoPhoenix (forge.rs G3). Leur binaire est hashé au boot et rechargé depuis ExoFS si compromis. Ne jamais stocker d'état critique uniquement dans la heap du driver.

 
Référence rapide — 30 erreurs — règle de vérification

Section de revue de code. Pour chaque erreur : une règle grep ou assert qui confirme qu'elle est couverte.

Réf.	Ver.	Cat.	Erreur	Règle de vérification
S1	v1	SILENCIEUSE	Spinlock dans handler IPI	grep 'lock()\|spin' kernel/src/exophoenix/*handler* → 0 résultat
S2	v1	SILENCIEUSE	verify_cap absent ExoFS	grep -r 'verify_cap' kernel/src/fs/exofs/syscall/ → 21 résultats min
S3	v1	SILENCIEUSE	SSR layout divergent A/B	Un seul fichier ssr.rs. grep 'SSR_BASE' → un seul point de définition.
S4	v1	SILENCIEUSE	PT walker récursif	walk_a_page_tables() contient uniquement des boucles for. Aucun appel récursif.
S5	v1	SILENCIEUSE	PMC snapshot = source de vérité	grep 'PMC.*score -=\|score.*-.*PMC' → 0 résultat. PMC contribue positif uniquement.
S6	v1	SILENCIEUSE	PKRS non sauvegardé au context switch	switch_context contient rdpkrs/wrpkrs conditionnels sur B_FEATURES.pks.
S7	v1	SILENCIEUSE	e820 sans réservation SSR	mark_reserved(SSR_BASE) précède TOUT init buddy. Grep pour l'ordre.
S8	v2	SILENCIEUSE	INVPCID local — TLB shootdown via IPI 0xF3	Après modif PTE : send_ipi_broadcast(0xF3) + wait_tlb_acks(). Pas d'INVPCID.
S9	v2	SILENCIEUSE	Relaxed sur champs critiques SSR	SSR_HANDOFF_FLAG, FREEZE_ACK, LIVENESS_NONCE : Release/Acquire uniquement.
S10	v2	SILENCIEUSE	APIC ID sparse → overflow SSR	grep 'apic_id \* ' kernel/src/exophoenix/ → 0. Uniquement apic_to_slot[id]*64.
S11	v2	SILENCIEUSE	#GP dans 0xF2 si PMC non supporté	handler_pmc() vérifie B_FEATURES.pmc_available avant tout rdmsr.
S12	v2	SILENCIEUSE	IPI broadcast non-atomique (fenêtre inhérente)	Limite documentée. Timeout 100µs couvre la fenêtre. scan_spinlocks() dans forge.
S13	v2	SILENCIEUSE	Stack B sans guard page	alloc_guarded_stack() appelle mark_page_not_present() sous la stack.
G1	v1	GRAVE	SIPI avant Stage 0 complet	send_sipi_once() est la DERNIÈRE instruction de stage0_init().
G2	v1	GRAVE	INIT IPI sans masquage MSI préalable	mask_all_msi_msix() précède send_init_ipi() dans begin_isolation_hard().
G3	v1	GRAVE	Reload driver sans device reset	Séquence forge : pci_flr → drain_dma → iotlb_flush → reload_binary.
G4	v1	GRAVE	Handoff sans drain IOMMU simultané	IPI 0xF1 ET iommu_soft_revoke_all() démarrent en même temps.
G5	v1	GRAVE	buddy is_empty(NULL)	if list_head.is_null() { return true; } en tête de is_empty().
G6	v2	GRAVE	CLI ne bloque pas les NMI	Limite documentée. Commentaire explicite dans handlers. NMI via IST[2].
G7	v2	GRAVE	TOCTOU sur verify_cap ExoFS	copy_from_user() est AVANT verify_cap() dans chaque handler ExoFS.
G8	v2	GRAVE	Double SIPI	send_sipi_once() utilise SIPI_SENT.fetch_or(). Retourne Err si déjà envoyé.
G9	v2	GRAVE	FACS RO non re-vérifié post-forge	Checklist reconstruct_kernel_a() : FACS RO + MADT hash + TLB shootdown + IDT check.
S-N1	v3	SILENCIEUSE	IOMMU IOTLB non flushed	iommu_iotlb_flush() présent après tout hard revoke. QI descriptor + completion wait.
S-N2	v3	SILENCIEUSE	SMI faux positifs Sentinel	elapsed > T_DETECTION * 3 → SMI_COUNTER++ + continue. Pas d'escalade.
G-N1	v3	GRAVE	IST non configuré	Tous handlers IPI/NMI/#PF ont set_stack_index() dans l'IDT. TSS avec 3 entrées IST.
G-N2	v3	GRAVE	APIC Timer non calibré	calibrate_apic_via_pit() exécuté en étape 7. TICKS_PER_US stocké avant tout timeout.
S-N3	v4	SILENCIEUSE	x2APIC non détecté	Toutes fonctions APIC utilisent apic_write/read avec dispatch B_FEATURES.apic_mode.
S-N4	v4	SILENCIEUSE	CPUID probe absent	B_FEATURES.init() est l'étape 0.5 de Stage 0, avant tout accès hardware.
G-N3	v4	GRAVE	VMXOFF absent si VT-x actif	if B_FEATURES.vmx_active { asm!(vmxoff); clear CR4.VMXE; } dans étape 8.
N-ACS	v4	IOMMU	PCIe ACS non activé	enable_pcie_acs() appelé dans étape 9 pour tous les Root Ports détectés.

ExoOS Guide d'Implémentation v2 — Complet
30 erreurs · Phases 0-5 · Code Rust réel · Règles de vérification
Phase 0 démarre maintenant — cargo check d'abord.
