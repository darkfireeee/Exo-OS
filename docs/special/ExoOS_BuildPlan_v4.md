ExoOS — Plan de Construction v4
Version Définitive · Dernières corrections · Prêt pour implémentation
Mars 2026 · ExoOS Project

0. Analyse froide — Ronde finale

Verdict global	KIMI	GPT5	GROK4	Z-AI	Résultat
AXE 1 — S-N1,S-N2,G-N1,G-N2	4/4 VALIDE	3/4 VALIDE, 1 PART.	4/4 VALIDE	4/4 VALIDE	✓ VALIDÉ
AXE 2 — S8 IPI, G6 CLI/NMI	VALIDE	VALIDE	VALIDE	VALIDE	✓ VALIDÉ
AXE 3 — Ordre Stage 0	CORRECT*	PART.*	CORRECT	CORRECT	✓ + 2 ajouts
AXE 4 — Nouvelles erreurs	5 ajouts	7 points	AUCUNE	AUCUNE	2 réels

Arbitrage : Grok4 et Z-AI = PRÊT sans réserve (convergence 2/4). KIMI = 5 ajouts dont 2 critiques réels. Gemini = 2 nouvelles erreurs dont 1 réelle. ChatGPT = liste de choses déjà documentées en Security Model.

Résultat de l'arbitrage froid : 4 intégrations réelles nécessaires avant de coder (x2APIC, CPUID probe, PCIe ACS, VMXOFF). Le reste appartient au Security Model / platform matrix. Après ces 4 intégrations, le plan est clos.

 
1. Intégrations finales — 4 points

S-N3	x2APIC mode non détecté — APIC MMIO silencieusement inopérant	SILENCIEUSE CRITIQUE (KIMI + Gemini)

Description (consensus 2/4)
•	Sur hardware moderne (Intel Skylake+, tout AMD EPYC/Ryzen serveur), le CPU peut être en mode x2APIC (activé par firmware BIOS/UEFI). En x2APIC, les accès MMIO à l'APIC local (lecture/écriture dans la page LAPIC à 0xFEE00000) sont silencieusement ignorés — les registres ne sont accessibles QUE via MSRs (IA32_X2APIC_*). Si B configure son APIC et envoie des IPIs via MMIO sans détecter ce mode, rien ne se passe. Watchdog ne s'arme pas. IPIs 0xF1/0xF2/0xF3 n'atteignent jamais les cores. Défaillance silencieuse totale.

Résolution S-N3 — Détection x2APIC au Stage 0 (étape 0.5)
1.	Détecter le mode APIC actuel via CPUID leaf 1 (bit 21 de ECX = x2APIC) ET via IA32_APIC_BASE MSR (bit 10 = x2APIC enabled).
fn detect_apic_mode() -> ApicMode {
    let ecx = cpuid!(1).ecx;
    let apic_base = rdmsr(IA32_APIC_BASE);
    if (ecx & (1 << 21) != 0) && (apic_base & (1 << 10) != 0) {
        ApicMode::X2Apic   // MSR-based
    } else {
        ApicMode::XApic    // MMIO-based (0xFEE00000)
    }
}

2.	Toutes les fonctions APIC de B (send_ipi, set_timer, read_id, send_eoi) doivent utiliser un dispatch conditionnel selon le mode détecté :
fn apic_write(reg: ApicReg, val: u32) {
    match APIC_MODE.load(Ordering::Relaxed) {
        ApicMode::XApic  => mmio_write(APIC_BASE + reg.offset(), val),
        ApicMode::X2Apic => wrmsr(reg.msr_addr(), val as u64),
    }
}

3.	APIC_MODE est une statique initialisée en étape 0.5 du Stage 0, avant tout accès APIC.
L'envoi d'IPI est aussi différent : en xAPIC, on écrit ICR low/high en deux passes. En x2APIC, un seul MSR IA32_X2APIC_ICR en 64 bits. Le format du vecteur IPI change aussi légèrement.

C-STAGE0-CPUID	Étape 0.5 ajoutée au Stage 0 — CPUID feature probe	AJOUT STAGE 0 (KIMI + ChatGPT)

Description
•	Le plan v3 commence directement par 'B installe ses page tables'. Mais plusieurs étapes ultérieures dépendent de features CPU qui doivent être détectées via CPUID AVANT d'être utilisées : INVPCID (S8 fallback), PKS (F1 Intel), invariant TSC (calibration), x2APIC (S-N3), PMC support (S11/E1), HPET (G-N2 fallback).
•	Sans ce probe initial, le code doit faire des hypothèses sur le hardware. Sur un hardware sans une de ces features, le code plante ou se comporte de façon indéterminée.

Résolution — Étape 0.5 insérée après étape 1
L'ordre Stage 0 révisé devient 14 étapes :

Étape	Action	Features détectées
1	B installe ses page tables	Aucune dépendance — premier
0.5 NEW	CPUID feature probe global	x2APIC, PKS, INVPCID, invariant TSC, PMC (leaf 0xA), HPET (ACPI), PCID, SMEP/SMAP. Stockés dans B_FEATURES statique.
2	Stack + guard page	—
3	IST dans TSS	—
4	IDT avec handlers stubs	—
5	Parse ACPI (MADT, FADT, FACS)	—
5.5	Énumération PCI (devices + BARs)	Nombre de devices → formule pool R3 V4-M2. Stocké dans B_DEVICE_TABLE.
6	Construire apic_to_slot[256]	—
7	Calibrer APIC Timer (PIT/HPET)	Utilise B_FEATURES.hpet_available pour choisir PIT ou HPET
8	Init APIC local (xAPIC ou x2APIC)	Utilise B_FEATURES.x2apic_active — mode détecté en 0.5. VMXOFF si VT-x actif.
9	IOMMU (VT-d / AMD-Vi) + ACS	Utilise B_DEVICE_TABLE (5.5). Active PCIe ACS. IOTLB flush via QI/Completion Wait.
10	FACS RO + hash MADT	—
11	Pool R3 (taille calculée en 5.5)	—
12	Armer watchdog APIC timer	—
13	PhoenixState::Normal + SIPI vers A	DERNIÈRE INSTRUCTION — toutes protections actives

G-N3	VMXOFF si VT-x actif — APIC IPI échoue en VMX root mode	GRAVE (KIMI G13)

Description (KIMI — seul)
•	Si le firmware a activé Intel VT-x (Virtualization Technology) et qu'ExoOS ne l'utilise pas (pas d'hyperviseur), certaines instructions se comportent différemment en 'VMX root operation mode'. INVVPID (invalidation TLB associée à VPID) est requis à la place d'INVPCID dans certains cas. Si B tente d'utiliser INVPCID en VMX root sans le flag approprié dans les VMCS, il déclenche un #GP.
•	ExoOS n'utilise pas d'hyperviseur. VT-x n'a aucune utilité. La solution propre est de désactiver VT-x au démarrage.

Résolution G-N3 — VMXOFF dans étape 8
4.	Dans l'étape 8 (APIC init), B vérifie si VT-x est actif via IA32_FEATURE_CONTROL MSR (bit 2 = VMXON dans SMX, bit 0 = VMXON hors SMX) et CR4.VMXE (bit 13).
// Dans étape 8 du Stage 0
let feat_ctrl = rdmsr(IA32_FEATURE_CONTROL);
let cr4 = read_cr4();
if cr4 & CR4_VMXE != 0 {
    // VT-x actif — émettre VMXOFF
    core::arch::asm!("vmxoff");
    // Effacer CR4.VMXE
    write_cr4(cr4 & !CR4_VMXE);
}

5.	Si VMXOFF échoue (RFLAGS.CF = 1) : VT-x était dans un état invalide. Continuer — ExoOS n'utilise pas VMX donc l'état est sans importance pour nous.
6.	Sur AMD : SVM (AMD-Vi virtualization) → vmsave/vmload non nécessaire, SVM non utilisé, aucune action requise.
VMXOFF ne doit pas paniquer si VT-x n'est pas actif — vérifier CR4.VMXE avant d'exécuter VMXOFF.

PCIe-ACS	PCIe ACS non activé — DMA peer-to-peer bypass IOMMU	NOTE PHASE 3.3b (Gemini)

Description (Gemini — seul)
•	PCIe ACS (Access Control Services) est un mécanisme qui empêche les devices PCIe de faire du DMA direct vers d'autres devices (peer-to-peer) en contournant l'IOMMU. Sans ACS activé sur les Root Ports et les Switches PCIe, un device compromis peut écrire dans la mémoire d'un autre device, contournant totalement les tables IOMMU que B a configurées.

Résolution — Intégration dans Phase 3.3b
7.	Dans la configuration IOMMU (étape 9 du Stage 0), B active ACS sur tous les Root Ports et PCIe Switches de la topologie détectée en 5.5.
8.	Méthode : pour chaque device PCI ayant une capability ACS (Extended Capability ID 0x000D), écrire dans ACS Control Register pour activer les bits SV (Source Validation), TB (Translation Blocking), RR (P2P Request Redirect), CR (P2P Completion Redirect).
9.	Si un device ne supporte pas ACS → IOMMU en mode strict pour ce device (isoler complètement dans son propre domaine IOMMU). Log dans B audit.
ACS est une opération one-time au Stage 0. Coût nul en runtime. Protection permanente.


2. Liste complète — 30 erreurs documentées (version finale)

12 originales (v1) + 8 ajoutées (v2) + 6 ajoutées (v3) + 4 ajoutées (v4) = 30 erreurs. Cette liste est close.

Réf.	Ver.	Catégorie	Erreur
S1	v1	SILENCIEUSE	Handler IPI avec spinlock → deadlock
S2	v1	SILENCIEUSE	verify_cap absent dans handlers ExoFS
S3	v1	SILENCIEUSE	SSR layout divergent A vs B
S4	v1	SILENCIEUSE	PT walker récursif → stack overflow B
S5	v1	SILENCIEUSE	PMC snapshot comme source de vérité
S6	v1	SILENCIEUSE	PKRS non sauvegardé au context switch
S7	v1	SILENCIEUSE	e820 sans réservation SSR → buddy alloue la SSR
G1	v1	GRAVE	SIPI vers A avant Stage 0 complet
G2	v1	GRAVE	INIT IPI sans masquage MSI/MSI-X préalable
G3	v1	GRAVE	Reload driver Ring 1 sans device reset
G4	v1	GRAVE	Handoff sans drain IOMMU simultané
G5	v1	GRAVE	Buddy allocator is_empty(NULL) → crash heap
S8	v2	SILENCIEUSE CRITIQUE	TLB shootdown IPI 0xF3 — INVPCID = local seulement
S9	v2	SILENCIEUSE CRITIQUE	Memory Ordering Relaxed au lieu de Acquire sur SSR
S10	v2	SILENCIEUSE CRITIQUE	APIC ID sparse → overflow SSR — table apic_to_slot
S11	v2	SILENCIEUSE GRAVE	#GP dans handler 0xF2 si PMC non supporté — CPUID leaf 0xA
S12	v2	SILENCIEUSE	IPI broadcast non-atomique — fenêtre 10-50µs inhérente
S13	v2	SILENCIEUSE	Stack overflow core B sans guard page
G6	v2	GRAVE (desc. corrigée v3)	CLI bloque IRQs maskables uniquement — NMI non-bloquable par software
G7	v2	GRAVE	TOCTOU ExoFS verify_cap — copy_from_user avant vérif
G8	v2	GRAVE	Double SIPI — bitset SIPI_SENT atomique
G9	v2	GRAVE	FACS RO non re-vérifié après restore — checklist ExoForge
S-N1	v3	SILENCIEUSE CRITIQUE	IOMMU IOTLB non flushed — QI Intel / Completion Wait AMD
S-N2	v3	SILENCIEUSE	SMI firmware — faux positif T_detection Sentinel
G-N1	v3	GRAVE CRITIQUE	IST non configuré — Triple Fault si RSP corrompu de A
G-N2	v3	GRAVE CRITIQUE	APIC Timer non calibré — tous timeouts faux sans PIT/HPET
S-N3	v4 NEW	SILENCIEUSE CRITIQUE	x2APIC mode non détecté — APIC MMIO silencieusement inopérant
S-N4	v4 NEW	AJOUT STAGE 0	CPUID feature probe absent — code utilise features sans vérifier disponibilité
G-N3	v4 NEW	GRAVE	VMXOFF absent si VT-x actif — #GP possible avec INVPCID en VMX root
N-ACS	v4 NEW	NOTE IOMMU	PCIe ACS non activé — DMA peer-to-peer bypass IOMMU


3. Stage 0 — Ordre final v4 (14 étapes)

Intégrant les corrections de toutes les rondes. Version figée.

Étape	Action	Contrainte / Dépendances
1	Page tables de B	Premier — zéro dépendance
0.5	CPUID feature probe (B_FEATURES)	NEW v4. Détecter : x2APIC, PKS, INVPCID, TSC invariant, PMC, HPET, PCID. Stocké en statique.
2	Stack + guard page	Avant toute logique — overflow = non-récupérable
3	IST dans TSS (3 entrées)	Avant IDT. IST[0]=0xF1/0xF3, IST[1]=#PF, IST[2]=NMI
4	IDT — 0xF1, 0xF2, 0xF3, #PF, NMI	Après IST. Stubs pour l'instant
5	Parse ACPI (MADT, FADT, FACS)	Avant APIC + IOMMU qui dépendent des tables ACPI
5.5	Énumération PCI + BARs	NEW v4. Nombre de devices → calcul taille pool R3. Stocké en B_DEVICE_TABLE.
6	Table apic_to_slot[256]	Après MADT — APIC IDs réels connus
7	Calibration APIC Timer	Via PIT ch.2 ou HPET (selon B_FEATURES). Stocke TICKS_PER_US.
8	Init APIC (xAPIC ou x2APIC) + VMXOFF	Utilise B_FEATURES.x2apic — mode conditionnel. VMXOFF si CR4.VMXE actif. LINT1 configuré.
9	IOMMU + PCIe ACS + IOTLB flush	Après ACPI + device table. Deny-by-default. ACS activé sur Root Ports.
10	FACS RO + hash MADT	Après page tables + ACPI parsé
11	Pool R3 (taille calculée en 5.5)	Après IOMMU — pool protégée DMA
12	Armer watchdog APIC timer	Après calibration (timeout correct)
13	PhoenixState::Normal + SIPI → A	DERNIÈRE INSTRUCTION. Fenêtre R1 fermée.


4. Verdict final — BuildPlan v4

Grok4 + Z-AI (2/4)	PRÊT DÉFINITIVEMENT — Aucune erreur critique restante.
Gemini (1/4)	PRÊT phases 0-3.2. x2APIC + PCIe ACS intégrés en v4.
KIMI (1/4)	5 ajouts dont 2 critiques réels (x2APIC, CPUID probe) intégrés. Reste = Security Model.
ChatGPT (1/4)	Points listés = déjà couverts en Security Model / platform matrix.
CONSENSUS FINAL	BuildPlan v4 est définitivement prêt. 30 erreurs documentées. Stage 0 en 14 étapes. Phase 0 peut démarrer immédiatement.

Ce qui reste dans le Security Model (non bloquant)
•	SMM Ring-2 : limite hardware documentée. Hors scope sans TXT/TPM.
•	Device firmware persistence (NIC/HBA) : F8 + FLR couvrent le driver. Firmware signé = platform requirement.
•	Micro-arch side-channels (Spectre, L1TF) : mitigations IBRS/STIBP/SSBD = platform matrix.
•	Physical attacks (JTAG, Thunderbolt) : hors scope software.
•	Update de B : B minimal, surface réduite. Update = reboot avec nouveau B signé.
•	S3/S4 suspend/resume : désactivés sur plateformes ExoPhoenix. FACS RO protège contre S3 accidentel.

Première ligne de code — Phase 0 maintenant
10.	cargo check : corriger libs/exo-alloc + toolchain nightly.
11.	TCB 256 bytes : étendre task.rs + static_assert.
12.	SSR layout : créer kernel/src/exophoenix/ssr.rs avec constantes v6.
13.	Phase 3.1 : IDT + TSS IST (3 entrées) + vecteurs 0xF1/0xF2/0xF3 réservés.

ExoOS BuildPlan v4 — Version définitive et close.
30 erreurs documentées · 14 étapes Stage 0 · Prêt à implémenter.
ExoOS Project · Plan de Construction v4.0 · Mars 2026
