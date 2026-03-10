EXO-BOOT
Stratégie d'Intégration & Politique de Démarrage
Quand utiliser exo-boot — Quand utiliser GRUB — Contrat BootInfo — Règles d'intégration
📋  Ce document définit la politique de démarrage d'ExoOS. Il est à lire EN PARALLÈLE du document 'Roadmap vers exo-boot' qui détaille les prérequis techniques.
 
1 — Deux chemins de démarrage permanents
✅  GRUB et exo-boot ne sont PAS en compétition. Ils coexistent définitivement — chacun a un domaine d'application précis qui ne change pas.

Chemin	Firmware	Machines concernées	Statut	Responsabilité
GRUB + Multiboot2	BIOS legacy	QEMU q35, VMs, machines legacy (pré-2013)	✅ Actif — premier boot réussi	Développement, CI, tests automatisés
exo-boot UEFI	UEFI ≥ 2.0	Machines modernes (2013+), hardware réel de production	🔴 À intégrer — prérequis non remplis	Production, hardware réel, Secure Boot
exo-boot BIOS	BIOS legacy	Aucune — décision Option A	❌ Abandonné — mbr.asm/stage2.asm orphelins	Suppression planifiée

🔴  Les fichiers exo-boot/src/bios/mbr.asm, stage2.asm, disk.rs et vga.rs sont orphelins. GRUB remplace fonctionnellement tout ce code avec 20 ans de stabilité. Ces fichiers seront supprimés lors de la Phase 5 — voir document Roadmap.

2 — GRUB + Multiboot2 : domaine permanent
2.1 — Ce que GRUB fait à la place d'exo-boot BIOS
Responsabilité	exo-boot BIOS (orphelin)	GRUB (actif)
Stage 1 MBR (512 bytes)	mbr.asm — à écrire	grub-core — stable depuis 20 ans
Activation A20 + mode protégé	stage2.asm — à écrire	Inclus — transparent
Passage en Long Mode 64 bits	stage2.asm — à écrire	Multiboot2 + notre trampoline .code32
Lecture kernel depuis disque	disk.rs (INT 13h) — à écrire	Gestion automatique ext4/FAT/btrfs
Passage des arguments kernel	Handoff BootInfo — à écrire	EAX=magic, EBX=mb2_info — fait
KASLR	Possible mais complexe en 32 bits	❌ Pas supporté — acceptable en dev
Vérification Ed25519	Possible mais complexe	❌ Pas supporté — acceptable en dev

2.2 — Règles d'utilisation permanentes de GRUB
📋  GRUB reste le bootloader de référence pour tout ce qui n'est pas du hardware réel UEFI de production. Ce n'est pas un état 'temporaire à remplacer'.

// RÈGLE GRUB-01 : GRUB est le chemin de développement PERMANENT
// Ne jamais supprimer le support Multiboot2 du kernel.
// Le trampoline 32→64 bits dans kernel/src/main.rs reste à vie.
 
// RÈGLE GRUB-02 : CI/CD utilise EXCLUSIVEMENT GRUB
// timeout 25 qemu-system-x86_64 -machine q35 -m 256M \
//   -cdrom exo-os.iso → exit=124 (HLT loop) = succès
// Attendu : XK12356ps789abcdefgZAIOK
 
// RÈGLE GRUB-03 : BootInfo étendue non disponible via GRUB
// Le kernel doit fonctionner sans entropie hardware (→ fallback TSC+RDRAND)
// Le kernel doit fonctionner sans GOP framebuffer (→ VGA texte ou port 0xE9)
// Le kernel doit fonctionner sans KASLR (→ adresse de chargement fixe)
 
// RÈGLE GRUB-04 : Makefile — deux targets obligatoires
// make iso         → grub-mkrescue → exo-os.iso (BIOS/GRUB)
// make uefi-image  → exo-boot.efi + kernel.elf → image UEFI (Phase 5)

3 — exo-boot UEFI : domaine et déclencheurs
3.1 — Ce qu'exo-boot apporte que GRUB ne peut pas donner
Fonctionnalité	GRUB	exo-boot UEFI	Impact si absent
Ed25519 sur kernel.elf avant chargement (BOOT-02)	❌	✅ verify_kernel_or_panic()	Kernel non authentifié en production — inacceptable
Entropie hardware 64 bytes (BOOT-05)	❌	✅ EFI_RNG_PROTOCOL	CSPRNG kernel sans seed sûre → cryptographie faible
KASLR réel [1GiB, 256GiB] (BOOT-07)	❌	✅ compute_kaslr_base()	Adresse kernel prévisible → exploits return-oriented
BootInfo structurée (BOOT-03)	Partiel (mb2_info brute)	✅ BootInfo repr(C) complète	Pas de framebuffer, pas d'ACPI RSDP propre
GOP Framebuffer natif	❌	✅ EFI_GRAPHICS_OUTPUT_PROTOCOL	Pas d'affichage boot sur UEFI sans GOP
ACPI RSDP depuis EFI Config Tables	Via scan BIOS (fragile)	✅ find_acpi_rsdp_uefi()	Scan BIOS peut échouer sur firmware UEFI moderne
Secure Boot firmware	Dépend du firmware	✅ Vérification variables EFI	Impossible sans exo-boot.efi signé

3.2 — Déclencheur d'activation exo-boot
📋  exo-boot UEFI s'active UNIQUEMENT quand les prérequis du document Roadmap sont tous cochés ET que les deux conditions suivantes sont remplies.

Condition	Description	Vérification
Hardware réel UEFI disponible	Une vraie machine ou une VM UEFI (OVMF) pour tester	qemu-system-x86_64 -bios /usr/share/OVMF/OVMF_CODE.fd
Kernel compile en ET_DYN (PIE)	apply_pie_relocations() requiert un kernel relocatable	file kernel.elf → ELF64 shared object (ET_DYN)

4 — Architecture dual-boot dans kernel_main()
4.1 — Détection du chemin de boot
Le kernel doit détecter quel bootloader l'a lancé. Le magic BOOT_INFO_MAGIC sert exactement à ça.

// kernel/src/arch/x86_64/boot/early_init.rs
 
const BOOT_INFO_MAGIC: u64 = 0x4F42_5F53_4F4F_5845; // 'EXOOS_BO' little-endian
const MB2_MAGIC: u32       = 0x36d7_6289;           // Multiboot2 magic
 
#[repr(C)]
pub enum BootPath {
    /// Chemin GRUB — mb2_magic=0x36d76289, mb2_info=pointeur Multiboot2
    Multiboot2 { magic: u32, info: *const u8 },
    /// Chemin exo-boot UEFI — boot_info validé par magic EXOOS_BO
    ExoBoot    { boot_info: *const BootInfo },
}
 
pub fn detect_boot_path(rdi: u64, rsi: u64) -> BootPath {
    // Chemin exo-boot : RDI = pointeur BootInfo (magic dans les 8 premiers bytes)
    let candidate = rdi as *const u64;
    if rdi >= 0x1000 && unsafe { candidate.read_volatile() } == BOOT_INFO_MAGIC {
        return BootPath::ExoBoot {
            boot_info: rdi as *const BootInfo,
        };
    }
    // Chemin Multiboot2 : RDI = mb2_magic (u32), RSI = mb2_info (u64)
    if (rdi as u32) == MB2_MAGIC {
        return BootPath::Multiboot2 {
            magic: rdi as u32,
            info: rsi as *const u8,
        };
    }
    // ❌ Aucun magic reconnu — panic kernel immédiat
    panic!("FATAL: Boot path inconnu — RDI={:#x} RSI={:#x}", rdi, rsi);
}

4.2 — Extraction des informations selon le chemin
// kernel/src/arch/x86_64/boot/early_init.rs
 
pub struct BootParams {
    pub memory_regions:  &'static [MemoryRegion],
    pub acpi_rsdp:       Option<u64>,
    pub entropy:         [u8; 64],   // Zéro si GRUB (→ fallback RDRAND+TSC)
    pub framebuffer:     Option<FramebufferInfo>,
    pub kaslr_base:      Option<u64>, // None si GRUB (adresse fixe)
    pub secure_boot:     bool,
}
 
pub fn extract_boot_params(path: &BootPath) -> BootParams {
    match path {
        BootPath::Multiboot2 { info, .. } => {
            BootParams {
                memory_regions: parse_mb2_memory_map(*info),
                acpi_rsdp:      scan_acpi_rsdp_bios(),  // scan 0xE0000-0xFFFFF
                entropy:        [0u8; 64],               // ⚠️ Fallback RDRAND+TSC
                framebuffer:    None,                    // TODO : fb VGA
                kaslr_base:     None,                   // Adresse fixe
                secure_boot:    false,
            }
        },
        BootPath::ExoBoot { boot_info } => {
            let bi = unsafe { &**boot_info };
            BootParams {
                memory_regions: &bi.memory_regions[..bi.memory_count as usize],
                acpi_rsdp:      Some(bi.acpi_rsdp).filter(|&r| r != 0),
                entropy:        bi.entropy,              // ✅ EFI_RNG_PROTOCOL
                framebuffer:    bi.framebuffer.into(),
                kaslr_base:     Some(bi.kernel_physical_base),
                secure_boot:    bi.boot_flags & BOOT_FLAG_SECURE_BOOT != 0,
            }
        }
    }
}

4.3 — Règles de synchronisation BootInfo
⚠️  Tout changement dans kernel_loader/handoff.rs (exo-boot) DOIT être synchronisé avec kernel/src/arch/x86_64/boot/early_init.rs. Le magic 0x4F42_5F53_4F4F_5845 est le contrat de compatibilité.
Champ BootInfo	Type	Comportement si GRUB	Comportement si exo-boot
magic	u64	Non présent — détecté par MB2_MAGIC	0x4F42_5F53_4F4F_5845 vérifié en premier
version	u32	N/A	BOOT_INFO_VERSION=1 — vérifier compatibilité
memory_regions	[MemoryRegion; 256]	Parsé depuis mb2_info	Fourni directement par exo-boot
acpi_rsdp	u64	Scan BIOS 0xE0000-0xFFFFF	EFI Config Tables — fiable
entropy	[u8; 64]	Zéro — fallback RDRAND+TSC dans kernel	EFI_RNG_PROTOCOL garanti
framebuffer	FramebufferInfo	None — pas de GOP via GRUB	GOP address + width + height + stride
kernel_physical_base	u64	Adresse fixe (linker script)	Base KASLR aléatoire
boot_flags	u32	0 (pas de flags)	KASLR_ENABLED | SECURE_BOOT_ACTIVE…

5 — Règles absolues de coexistence
// RÈGLE COEX-01 : Le kernel NE DOIT PAS avoir de comportement conditionnel
// basé sur le chemin de boot pour les fonctions critiques.
// La sécurité (capabilities, memory protection) est identique GRUB ou exo-boot.
 
// RÈGLE COEX-02 : GRUB n'active PAS KASLR.
// Le kernel doit être compilé sans KASLR pour le chemin GRUB.
// Conséquence : les adresses kernel sont prévisibles en dev. Acceptable.
// INTERDIT en production (→ exo-boot UEFI obligatoire).
 
// RÈGLE COEX-03 : L'entropie GRUB est dégradée.
// entropy = [0; 64] → le kernel initialise son CSPRNG avec RDRAND + TSC.
// INTERDIT d'utiliser ce CSPRNG pour des opérations cryptographiques
// en production sans exo-boot (pas d'entropy de qualité EFI_RNG).
 
// RÈGLE COEX-04 : Le make iso BIOS et make uefi-image coexistent.
// Les deux DOIVENT passer en CI. Un seul ne suffit pas.
 
// RÈGLE COEX-05 : BootInfo est align(4096) dans exo-boot (static mut).
// Lors de la détection dans le kernel, vérifier l'alignement du pointeur
// avant tout accès : assert!(rdi % 4096 == 0) si BOOT_INFO_MAGIC détecté.
 
// RÈGLE COEX-06 : Après ExitBootServices, aucun appel EFI Runtime Service
// sauf GetTime/SetTime/ResetSystem. Le kernel ne doit JAMAIS appeler
// les Boot Services après handoff (BOOT_SERVICES_ACTIVE = false).

🔴  RÈGLE CRITIQUE exo-boot spécifique à ExoFS : exo-boot charge kernel.elf via EFI_FILE_PROTOCOL (FAT32/ESP). ExoFS n'est PAS disponible au boot — exo-boot ne connaît pas ExoFS. Le kernel.elf doit être sur la partition ESP (FAT32), pas sur la partition ExoFS. La racine ExoFS est montée par le kernel APRÈS le handoff.

6 — Timeline d'activation exo-boot
Phase	Bootloader actif	Déclencheur de transition	Condition bloquante
Aujourd'hui (Phase 1-2)	GRUB uniquement	—	Mémoire virtuelle + heap non implémentées
Phase 3-4 (Kernel stable)	GRUB uniquement	—	ExoFS, syscalls, process, signal non actifs
Phase 5 (Userspace stable)	GRUB + tests exo-boot UEFI en parallèle	Premier boot exo-boot UEFI sur QEMU OVMF	kernel compilé ET_DYN + dual entry point
Phase 6 (Production)	GRUB (dev/CI) + exo-boot UEFI (prod)	Hardware réel UEFI testé	Signature Ed25519 kernel opérationnelle
Phase 7 (Nettoyage)	Idem Phase 6	—	Suppression mbr.asm, stage2.asm, disk.rs, vga.rs d'exo-boot

ℹ️  La séquence boot actuelle (XK12356ps789abcdefgZAIOK → halt_cpu) est celle de Phase 1. exo-boot ne sera jamais utilisé avant que le kernel ait quelque chose d'utile à faire après kernel_main().
