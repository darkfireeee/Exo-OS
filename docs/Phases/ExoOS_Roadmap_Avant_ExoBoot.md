ROADMAP EXO-OS
Prérequis complets avant intégration exo-boot
Mémoire · Scheduler · ExoFS · IPC · Process · Signal · Userspace · Erreurs silencieuses
⚠️  Point de départ : premier boot réussi (5 mars 2026) — séquence XK12356ps789abcdefgZAIOK → halt_cpu(). Aucun heap, aucune mémoire virtuelle, aucun scheduler actif, aucun ExoFS, aucun userspace. Ce document liste TOUT ce qui doit être fait avant d'activer exo-boot UEFI.
 
1 — Vue d'ensemble : 7 phases avant exo-boot
Phase	Modules	Condition de sortie	Bloquant pour exo-boot
Phase 1 Mémoire virtuelle	memory/virtual/, memory/heap/, memory/swap/compress.rs	Heap kernel opérationnelle, PML4 haute mémoire, APIC remappé avec NX+UC	✅ Oui — tout dépend de la heap
Phase 2 Scheduler + IPC de base	scheduler/, ipc/core/, ipc/ring/, ipc/sync/futex.rs	Context switch fonctionnel, SPSC ring testé, Futex opérationnel	✅ Oui — process/ requiert scheduler
Phase 3 Process + Signal	process/lifecycle/, process/signal/, signal/, syscall/handlers/	fork/exec/exit fonctionnel, signal delivery testé, syscalls 0-50 opérationnels	✅ Oui — userspace impossible sans ça
Phase 4 ExoFS	fs/exofs/ complet, crypto pipeline, epoch/commit	Montage ExoFS, lecture/écriture objet, syscalls 500-519 opérationnels	✅ Oui — init server dépend d'ExoFS
Phase 5 Servers Ring 1	servers/init, servers/ipc_broker, servers/vfs_server	PID 1 (init) démarre et supervise services	✅ Oui — exo-boot nécessite un OS fonctionnel
Phase 6 Exo-boot UEFI	exo-boot + kernel dual-entry, BootInfo contract	Boot UEFI sur QEMU OVMF réussi	— C'EST L'OBJECTIF
Phase 7 Nettoyage	Suppression code BIOS orphelin d'exo-boot	mbr.asm, stage2.asm, disk.rs supprimés	—

2 — Phase 1 : Mémoire virtuelle et heap kernel
🔴  PHASE BLOQUANTE ABSOLUE. Rien d'autre ne peut avancer sans la heap kernel. Le buddy allocator et le slab allocator sont des prérequis de tout le reste.

2.1 — Ordre impératif des tâches mémoire
Étape	Module	Dépend de	Condition de validation
2.1.1	Finaliser PML4 kernel haute mémoire memory/virtual/page_table/x86_64.rs	Trampoline boot (fait)	Kernel accessible via KERNEL_HIGHER_HALF_BASE = 0xFFFF_FFFF_8000_0000
2.1.2	Remappe APIC MMIO avec attributs corrects memory/virtual/address_space/mapper.rs	2.1.1	LAPIC 0xFEE00000 → VMA kernel avec NX+UC IOAPIC 0xFEC00000 → VMA kernel avec NX+UC
2.1.3	Remappe HPET MMIO arch/x86_64/acpi/hpet.rs (init différée)	2.1.1	HPET 0xFED00000 → VMA kernel avec NX+UC Init HPET complète (était différée)
2.1.4	Buddy allocator opérationnel memory/physical/allocator/buddy.rs	2.1.1	alloc_frame() / free_frame() sans panic Test : 1000 alloc/free consécutifs stables
2.1.5	Slab/SLUB allocator memory/heap/allocator/hybrid.rs	2.1.4	#[global_allocator] actif Test : Box::new(42u64) sans panic
2.1.6	VMA tree (RBTree intrusive) memory/virtual/vma/tree.rs	2.1.5	mmap/munmap/mprotect sur espaces vides
2.1.7	Compression swap LZ4 memory/swap/compress.rs	2.1.5	Compress/decompress page 4KB aller-retour Ratio > 1 sur données réelles

2.2 — Erreurs silencieuses mémoire à prévenir
🔴  ERREUR SILENCIEUSE MEM-01 : Les pages APIC MMIO sont actuellement mappées P|R/W|PS sans NX et sans UC (Write-Back). Sur hardware réel, l'accès Write-Back au LAPIC donne des comportements non-déterministes (valeurs lues fausses, interruptions perdues). QEMU tolère ça — le vrai matériel non.

// kernel/src/memory/virtual/address_space/mapper.rs
// À faire en PRIORITÉ 1 après activation PML4 haute mémoire
 
// ❌ ÉTAT ACTUEL (trampoline de boot — temporaire) :
// PD_high[502] = 0xFEC00083  // P | R/W | PS — PAS UC, PAS NX
// PD_high[503] = 0xFEE00083  // P | R/W | PS — PAS UC, PAS NX
 
// ✅ ÉTAT REQUIS après init_memory_integration() :
pub const PAT_UC: u64 = 1 << 7;  // Page Attribute Table : Uncacheable
pub const PTE_NX: u64 = 1 << 63; // No-Execute
 
// Remap LAPIC : phys 0xFEE00000 → virt LAPIC_VIRT_BASE
// Flags : Present | R/W | NX | UC (PAT bit + PCD + PWT)
map_mmio_region(
    phys: 0xFEE00000,
    virt: LAPIC_VIRT_BASE,
    size: 0x1000,  // 4KB suffisant (pas 2MB huge page pour MMIO)
    flags: PAGE_PRESENT | PAGE_RW | PAGE_NX | PAGE_PCD | PAGE_PWT,
);
// ⚠️  Invalider le TLB après remap : invlpg(LAPIC_VIRT_BASE)
// ⚠️  Mettre à jour local_apic.rs pour utiliser LAPIC_VIRT_BASE
//     et non plus l'adresse physique 0xFEE00000

🔴  ERREUR SILENCIEUSE MEM-02 : Le buddy allocator ne doit PAS allouer des pages dans les régions ACPI, MMIO, ou au-dessus de la mémoire physique réelle. Sans parsing correct de la MemoryMap Multiboot2, une allocation peut corrompre une table ACPI ou le firmware BIOS. Valider chaque MemoryRegion contre la map avant de l'ajouter au pool.

⚠️  ERREUR SILENCIEUSE MEM-03 : Le TSC calibré à 1 GHz en fallback (QEMU) donne des délais faux sur hardware réel. Les sleep/timeout basés sur le TSC dans scheduler/timer/ seront incorrects. À corriger en Phase 2 : implémenter la calibration TSC via HPET (après 2.1.3) ou PM Timer (ACPI).

3 — Phase 2 : Scheduler et IPC de base
3.1 — Scheduler
Étape	Module	Règle critique	Erreur silencieuse à éviter
Context switch x86_64	scheduler/asm/switch_asm.s	r15 DOIT être préservé (callee-saved) XSAVE/XRSTOR si FPU utilisée	Sans XSAVE : état FPU corrompu entre threads → résultats float faux, silencieux
RunQueue CFS	scheduler/core/runqueue.rs	intrusive linked list — ZÉRO alloc dans ISR (SCHED-08 : WaitNode depuis EmergencyPool)	Allocation dans ISR → deadlock EmergencyPool → freeze kernel sans message d'erreur
Timer hrtimer	scheduler/timer/hrtimer.rs	Init APRÈS HPET disponible (Phase 1.3) Ne pas utiliser TSC 1GHz fallback en production	Timeout HPET basé sur 1GHz TSC = durées fausses Silencieux car QEMU TCG n'est pas temps-réel
Per-CPU state	scheduler/smp/topology.rs	SWAPGS obligatoire à chaque entrée/sortie Ring0 Kernel GS = percpu base, User GS = TLS	SWAPGS manquant → gs.base pointe vers données user → corruption percpu struct, crash différé
Preemption	scheduler/core/preempt.rs	preempt_disable() avant section critique preempt_enable() DOIT être appelé en sortie	preempt_enable() oublié → scheduler bloqué Silencieux jusqu'au premier timeout CPU

3.2 — IPC ring SPSC/MPMC
⚠️  ERREUR SILENCIEUSE IPC-01 : SPSC sans CachePadded → false sharing entre head (producteur) et tail (consommateur). Sur QEMU monocore invisible, sur hardware 8+ cœurs : dégradation 10-100x silencieuse. Le struct CachePadded (#[repr(C, align(64))]) est OBLIGATOIRE.

// kernel/src/ipc/ring/spsc.rs — CORRECT
#[repr(C, align(64))]
struct CachePadded<T> { value: T, _pad: [u8; 64 - size_of::<T>() % 64] }
 
pub struct SpscRing<T, const N: usize> {
    head: CachePadded<AtomicU64>,  // ✅ Cache line PRODUCTEUR séparée
    tail: CachePadded<AtomicU64>,  // ✅ Cache line CONSOMMATEUR séparée
    buffer: [MaybeUninit<T>; N],
}
// static_assert!(N est une puissance de 2 — MASK = N-1 pour modulo rapide)
// static_assert!(N <= 65536 — ring trop grand = cache thrashing)
 
// ❌ INTERDIT : head et tail dans le même struct sans CachePadded
// struct SpscRingBad { head: AtomicU64, tail: AtomicU64, ... }
// → false sharing garanti → dégradation silencieuse en multicore

⚠️  ERREUR SILENCIEUSE IPC-02 : Futex avec table SipHash non keyed (clé nulle ou fixe). Un attaquant Ring 3 peut forger des adresses pour créer des collisions HashDoS dans la futex_table et provoquer une attente infinie côté kernel. La clé DOIT être initialisée depuis security::crypto::rng à l'init mémoire.

4 — Phase 3 : Process management et signal
4.1 — Erreurs silencieuses process
🔴  ERREUR SILENCIEUSE PROC-01 (BUG-04 confirmé) : do_exec() ne contient pas l'initialisation du registre %fs (TLS base). ARCH_SET_FS ou wrmsrl(IA32_FS_BASE, tls_addr) est absent entre le chargement ELF et le jump vers l'entrée. Tout accès TLS dans exo-libc (__errno_location, pthread keys) cause un segfault immédiat. À corriger AVANT tout test userspace.

// kernel/src/process/lifecycle/exec.rs — do_exec() — CORRECTION REQUISE
// À insérer ENTRE step 7 (setup stack) et jump_to_entry :
 
// ✅ ÉTAPE MANQUANTE : initialiser %fs pour TLS
// L'adresse TLS est fournie dans le segment PT_TLS ou via AT_SIGNAL_TCB
let tls_base = allocate_initial_tls_block(tcb)?;
unsafe {
    // wrmsrl(IA32_FS_BASE = 0xC0000100, tls_base)
    core::arch::x86_64::__wrfsbase(tls_base);
    // OU via syscall ARCH_SET_FS si on veut passer par la couche syscall
}
tcb.fs_base = tls_base;
 
// ⚠️  Sans ça : exo-libc errno crashe dès le premier syscall Ring3
// ⚠️  Sans ça : __tls_get_addr() dans les .so reloc crashe aussi

🔴  ERREUR SILENCIEUSE PROC-02 (BUG-05 confirmé) : SYSRETQ sans vérification canonique de RCX. Si userspace met une adresse non-canonique dans RCX avant SYSCALL, le SYSRETQ faulte en Ring 0. Exploitable pour élévation de privilèges. Intel/AMD errata documenté. is_canonical(rcx) DOIT être vérifié avant SYSRETQ.

// kernel/src/syscall/trampoline.asm — CORRECTION REQUISE
// Avant SYSRETQ, vérifier que RCX (retour userspace) est canonique
 
// Adresse canonique x86_64 : bits 63:48 == bits 47 (sign extension)
// Non-canonique → #GP fault en Ring0 lors du SYSRETQ
 
// ✅ Vérification en assembleur :
// mov  rax, rcx
// sar  rax, 47          ; propagation du bit 47
// test rax, rax         ; si 0 ou -1 → canonique
// jz   .canonical_ok
// cmp  rax, -1
// je   .canonical_ok
// ; ❌ Non-canonique → SIGSEGV au process, NE PAS faire SYSRETQ
// jmp  .deliver_sigsegv
// .canonical_ok:
// swapgs
// sysretq

4.2 — Signal : règles de cohérence Ring 0
Règle	Module	Erreur silencieuse si violée
SIG-01 : SigactionEntry stocke VALEUR, jamais AtomicPtr	signal/tcb.rs	AtomicPtr → déréférencement userspace depuis Ring0 → exploit Silencieux car QEMU ne valide pas les accès kernel
PROC-03 : Bloquer tous les signaux sauf SIGKILL pendant exec()	process/lifecycle/exec.rs	Signal livré entre load ELF et reset TCB → handler pointe vers l'ancien code → corruption silencieuse de l'espace d'adressage
SIG-13 : Vérifier magic 0x5349474E au sigreturn	signal/trampoline.asm	Sans vérification : attaquant forge un faux frame sigreturn pour écraser RIP arbitrairement — silent privilege escalation
SIG-07 : SIGKILL/SIGSTOP non-masquables	signal/mask.rs	Si masquables : processus zombie impossible à tuer Silencieux jusqu'à saturation de la table de processus
FORK-02 : page_table lock PENDANT mark_all_pages_cow + TLB shootdown	process/lifecycle/fork.rs	Race condition CoW : deux CPUs écrivent simultanément la même page → corruption données sans signal d'erreur

5 — Phase 4 : ExoFS — attention particulière requise
🔴  ExoFS N'EST PAS un filesystem classique. C'est un système d'objets content-addressed avec capabilities, epochs, déduplication et chiffrement. Les règles standard des FS POSIX NE s'appliquent pas directement. Cette section liste les pièges spécifiques à ExoFS.

5.1 — Pipeline crypto obligatoire — ordre non négociable
⚠️  CRYPTO-02 (règle absolue) : l'ordre DOIT être données→Blake3(BlobId)→compression→chiffrement→disque. JAMAIS compresser après chiffrement (ciphertext incompressible). JAMAIS calculer le BlobId après compression (BlobId change si algorithme de compression change).

// fs/exofs/io/blob_writer.rs — ordre OBLIGATOIRE
 
// ✅ CORRECT :
let blob_id    = blake3::hash(raw_data);          // Étape 1 : BlobId sur données brutes
let compressed = lz4_flex::compress(raw_data);    // Étape 2 : compression
let ciphertext = xchacha20::encrypt(             // Étape 3 : chiffrement
    &compressed,
    &derive_object_key(&master_key, &blob_id),   // Clé dérivée du BlobId
    &generate_nonce_from_counter(&blob_id),      // Nonce atomique (CRYPTO-NONCE-01)
);
write_to_disk(&ciphertext, &blob_id);            // Étape 4 : stockage
 
// ❌ INTERDIT — compresser après chiffrement :
// let ciphertext = encrypt(raw_data);
// let compressed = lz4_compress(ciphertext);  // ciphertext incompressible → ratio 1:1
 
// ❌ INTERDIT — BlobId après compression :
// let compressed = compress(raw_data);
// let blob_id = blake3::hash(compressed);    // BlobId change si algo compression change

5.2 — Nonce XChaCha20 — erreur silencieuse critique
🔴  ERREUR SILENCIEUSE CRYPTO-03 : Réutilisation de nonce avec XChaCha20-Poly1305 → destruction totale de la confidentialité (two-time pad). RDRAND+TSC seul NE garantit PAS l'unicité si deux threads génèrent des nonces simultanément. SEULE solution correcte : compteur atomique global + HKDF.

// fs/exofs/crypto/xchacha20.rs — génération nonce CORRECTE
 
// ✅ CORRECT : compteur atomique + HKDF
static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);
 
fn generate_nonce(object_id: &ObjectId) -> [u8; 24] {
    let counter = NONCE_COUNTER.fetch_add(1, Ordering::SeqCst);
    // HKDF dérive un nonce unique à partir du compteur + objet
    let mut nonce = [0u8; 24];
    hkdf::Hkdf::<sha2::Sha256>::new(Some(&object_id.0), &counter.to_le_bytes())
        .expand(&[], &mut nonce).expect("HKDF expand");
    nonce
}
 
// ❌ INTERDIT — RDRAND seul :
// fn generate_nonce() -> [u8; 24] {
//     let mut n = [0u8; 24];
//     rdrand_fill(&mut n);  // Race condition si 2 threads appelés simultanément
//     n                     // avec même seed TSC → nonce identique possible
// }

5.3 — Capabilities : erreurs silencieuses
Règle	Erreur silencieuse si violée	Correction
CAP-01 : verify() doit être constant-time (LAC-01 identifié)	Attaquant mesure différence de temps entre token révoqué (objet existe) et token inexistant (objet absent) → oracle pour énumérer les ObjectIds valides	verify() renvoie la même durée quelle que soit la raison du refus — pas de return early
CAP-03 : CapToken générations — révocation O(1)	Sans génération : un token révoqué peut être réutilisé si la table CapToken ne vérifie pas la génération Silencieux : access looks valid	generation_counter++ à chaque revoke() Comparaison génération stockée vs génération token
SEC-07 : verify_cap() AVANT toute opération ExoFS (SYS-07 pour les syscalls 500-519)	Sans verify_cap() : un process sans droits READ peut lire des objets — escalade de privilèges silencieuse	check_access() en premier dans CHAQUE handler CI grep : aucun handler sans verify()
LOBJ-01 : SYS_EXOFS_PATH_RESOLVE retourne ObjectId jamais un chemin kernel interne	Retourner un pointeur interne → leak d'adresse kernel Exploitable pour KASLR bypass	ObjectId est opaque (u128 content-addressed) JAMAIS retourner adresse kernel raw

5.4 — Epoch/commit — cohérence ExoFS
⚠️  ERREUR SILENCIEUSE EPOCH-01 : Un crash entre deux écritures d'un même Epoch laisse l'Epoch dans un état partiellement écrit. Sans journal d'Epoch, la recovery au redémarrage peut restaurer un état incohérent sans le détecter. Le checksum de l'EpochRecord (Blake3) doit être vérifié au montage.

// fs/exofs/epoch/recovery.rs — validation au montage
 
// ✅ Vérification checksum EpochRecord au montage :
fn verify_epoch_record(record: &EpochRecord) -> Result<(), EpochError> {
    // static_assert!(size_of::<EpochRecord>() == 104); // zerocopy garantit ça
    let expected = blake3::hash(&record.as_bytes()[..96]); // 96 bytes sans le checksum
    if record.checksum != expected.as_bytes()[..32] {
        return Err(EpochError::ChecksumMismatch);
        // → Lance recovery phase 1 (fsck)
    }
    if record.magic != EPOCH_RECORD_MAGIC {
        return Err(EpochError::MagicInvalid);
    }
    Ok(())
}
 
// ⚠️  EPOCH-05 : max 500 objets par Epoch
// Au-delà : forcer un commit implicite AVANT d'accepter le nouvel objet
// Silencieux si non vérifié : Epoch qui grossit sans limite → OOM kernel

5.5 — Syscall 519 (SYS_EXOFS_OPEN_BY_PATH) — BUG-01 critique
🔴  BUG-01 (confirmé) : musl-exo appelle open() avec UN seul syscall. ExoFS nécessite DEUX syscalls (PATH_RESOLVE=500 puis OBJECT_OPEN=501). Sans SYS_EXOFS_OPEN_BY_PATH=519 (syscall combiné), TOUTES les applications POSIX échouent silencieusement à l'open(). C'est le bug le plus bloquant pour le userspace.

// kernel/src/fs/exofs/syscall/open_by_path.rs — À CRÉER
// SYS_EXOFS_OPEN_BY_PATH = 519
 
// ✅ Syscall combiné : PATH_RESOLVE + OBJECT_OPEN atomique
pub fn sys_exofs_open_by_path(
    path_ptr: *const u8,
    path_len: usize,
    flags: u32,
    cap_token: CapToken,
) -> Result<FileDescriptor, ExofsError> {
    // 1. copy_from_user() OBLIGATOIRE (SYS-01)
    let path = copy_string_from_user(path_ptr, path_len)?;
    // 2. Résolution chemin → ObjectId (interne, jamais exposé)
    let object_id = path_index::resolve(&path)?;
    // 3. verify_cap() AVANT ouverture (SYS-07)
    security::verify_cap(&cap_token, &object_id, Rights::READ)?;
    // 4. Ouverture et retour fd
    Ok(fd_table::open(object_id, flags))
}
 
// ⚠️  musl-exo : syscall_arch.h → __NR_open = 519 (pas 2)
// ⚠️  __NR_getdents64 = 520 (BUG-02) — à définir

6 — Phase 5 : Servers Ring 1 — prérequis avant exo-boot
6.1 — Ordre de démarrage des servers
Ordre	Server	Prérequis kernel	Rôle critique
1	servers/ipc_broker (PID 2)	IPC ring fonctionnel capabilities actives	Directory service — TOUS les autres servers passent par lui pour se localiser (SRV-05)
2	servers/init (PID 1)	ipc_broker disponible signal SIGCHLD opérationnel	Supervise TOUS les services Restart automatique si crash (supervisor.rs)
3	servers/vfs_server (PID 3)	ExoFS monté PATH_RESOLVE fonctionnel	Namespaces de montage /proc, /sys, /dev via pseudo_fs/
4	servers/crypto_server (PID 4)	vfs_server disponible entropie CSPRNG opérationnelle	SRV-04 : seul service de confiance crypto Tous les autres délèguent ici
5	servers/net_stack, power_manager, login_manager…	crypto_server disponible	Services secondaires — non bloquants pour exo-boot

6.2 — Erreurs silencieuses Ring 1
⚠️  ERREUR SILENCIEUSE SRV-01 : Si un server Ring 1 crashe sans que init ait de handler SIGCHLD configuré, le processus devient zombie en silence. La table des processus sature progressivement. Implémenter supervisor.rs AVANT de lancer les servers en production.

⚠️  ERREUR SILENCIEUSE SRV-02 : Un server qui implémente sa propre crypto (en violation de SRV-04) utilisera des clés non-synchronisées avec crypto_server. Données chiffrées illisibles après redémarrage. CI grep obligatoire : aucun server sauf crypto_server n'importe chacha20poly1305 ou blake3.

7 — Catalogue des erreurs silencieuses transversales
📋  Ces erreurs ne crashent pas le système immédiatement. Elles produisent des comportements incorrects qui ne se manifestent qu'en production ou sous charge. Chacune nécessite une vérification active.

ID	Module	Erreur silencieuse	Symptôme tardif	Correction
ERR-01	arch/x86_64/cpu/tsc.rs	TSC calibré à 1GHz fallback (QEMU) Fréquence fausse sur hardware réel	Timeouts 10x trop courts ou longs Système inutilisable sur vrai matériel	Calibrer TSC via HPET (Phase 1.3) ou PM Timer ACPI après init mémoire
ERR-02	memory/virtual/ APIC remap	Pages APIC sans UC (Write-Back) 0xFEE00000 et 0xFEC00000	Interruptions perdues sur hardware réel APIC timer drift, IPI non livrées	Remap avec PCD+PWT (UC) après init PML4 (Phase 1.1)
ERR-03	security/crypto/rng.rs	Entropie GRUB = [0; 64] CSPRNG initialisé avec zéros	Clés de session prévisibles CSPRNG reproductible entre boots	Implémenter fallback RDRAND+TSC Dans kernel init avant heap (Phase 1)
ERR-04	ipc/ring/spsc.rs	CachePadded absent head/tail sur même cache line	Dégradation 10-100x sur multicore Invisible sur QEMU monocore	#[repr(C, align(64))] obligatoire Tester sur QEMU -smp 4 minimum
ERR-05	memory/utils/ futex_table.rs	Clé SipHash = 0 (non initialisée) Vulnérable HashDoS	Attaque userspace → freeze kernel Collisions O(n) → CPU 100%	Initialiser clé depuis CSPRNG avant toute utilisation futex
ERR-06	process/lifecycle/ exec.rs	%fs non initialisé avant jump (BUG-04)	Segfault immédiat tout programme exo-libc — visible mais mal diagnostiqué	wrmsrl(IA32_FS_BASE, tls_addr) entre setup_stack et jump_to_entry
ERR-07	syscall/trampoline.asm	RCX non-canonique avant SYSRETQ (BUG-05)	#GP fault Ring0 exploitable → privilege escalation	is_canonical(rcx) avant SYSRETQ Sinon → SIGSEGV au process
ERR-08	exofs/crypto/ xchacha20.rs	Nonce RDRAND seul sans atomique Deux threads → même nonce possible	Two-time pad XChaCha20 → confidentialité détruite	compteur AtomicU64 + HKDF Jamais RDRAND seul pour nonce
ERR-09	exofs/compress/ zstd_wrapper.rs	zstd-safe sans shims malloc ZSTD_malloc non défini	Undefined symbol à link OU crash runtime Ring0	ZSTD_malloc/ZSTD_free custom OU feature zstd_pure (ruzstd)
ERR-10	exofs/path/ path_index.rs	mount_secret_key = [0; 16] (non initialisée)	PathIndex vulnérable HashDoS par collision SipHash calculée	Initialiser depuis CSPRNG au montage Documenter dans key_storage.rs
ERR-11	process/signal/ delivery.rs	Signal livré pendant exec() entre load ELF et reset TCB	Handler pointe vers ancien code Corruption espace adressage silencieuse	block_all_except_kill() avant load_elf() dans do_exec()
ERR-12	exofs/epoch/ recovery.rs	EpochRecord sans vérification checksum au montage	Données corrompues after-crash restaurées sans détection	Blake3 checksum sur EpochRecord Vérifier au montage (fsck phase 1)

8 — Checklist finale — 32 points avant d'activer exo-boot
Phase 1 — Mémoire (obligatoire)
•	☑️ PML4 kernel haute mémoire activé (0xFFFF_FFFF_8000_0000)
•	☑️ APIC MMIO remappé avec NX + UC (PCD+PWT) — ERR-02
•	☑️ HPET init différée complétée après remap mémoire
•	☑️ Buddy allocator : alloc_frame/free_frame sans panic (1000 cycles test)
•	☑️ Slab/SLUB : #[global_allocator] actif — Box::new() fonctionnel
•	☑️ VMA tree (RBTree intrusive) — mmap/munmap fonctionnels
•	☑️ CSPRNG initialisé avec RDRAND+TSC (fallback GRUB) — ERR-03
•	☑️ TSC calibré via HPET ou PM Timer (pas fallback 1GHz) — ERR-01

Phase 2 — Scheduler + IPC (obligatoire)
•	☑️ Context switch x86_64 avec XSAVE/XRSTOR FPU
•	☑️ RunQueue intrusive — zéro alloc dans ISR (SCHED-08)
•	☑️ Timer hrtimer basé sur HPET calibré (ERR-01 corrigé)
•	☑️ SPSC ring avec CachePadded — implémenté (test -smp 4 à ajouter) — ERR-04
•	☑️ Futex table avec clé SipHash depuis CSPRNG — ERR-05
•	☑️ SWAPGS correct à chaque entrée/sortie Ring0

Phase 3 — Process + Signal (obligatoire)
•	☑️ do_exec() : %fs initialisé avant jump (BUG-04) — ERR-06
•	☑️ SYSRETQ : is_canonical(rcx) avant retour (BUG-05) — ERR-07
•	☑️ Signal blocking pendant exec() entre load_elf et reset_tcb — ERR-11
•	☑️ SigactionEntry : VALEUR pas AtomicPtr (SIG-01)
•	☑️ Magic 0x5349474E vérifié au sigreturn (SIG-13) — constant-time (LAC-01)
•	☑️ SIGKILL/SIGSTOP non-masquables (SIG-07)
•	☑️ fork() CoW : page_table lock + TLB shootdown atomique (FORK-02)
•	☑️ sys_fork câblé via do_fork() + fork_child_trampoline (iretq Ring3)
•	☑️ sys_execve câblé via do_execve() + frame RIP/RSP mis à jour
•	☑️ sys_rt_sigreturn câblé — magic vérifié, registres restaurés
•	☑️ sys_rt_sigaction câblé — lecture/écriture PCB.sig_handlers
•	☑️ sys_rt_sigprocmask câblé — masque TCB, SIGKILL/SIGSTOP non-masquables
•	☑️ Signal delivery câblée — post_dispatch → handle_pending_signals
•	☑️ sys_wait4 câblé → do_waitpid() ; wstatus POSIX ; WNOHANG/ECHILD/EINTR
•	☑️ sys_waitid câblé → do_waitpid() ; siginfo_t x86_64 rempli
•	☑️ sys_kill câblé → send_signal_to_pid() ; ESRCH/EPERM
•	☑️ sys_tgkill câblé → send_signal_to_tcb() ; SigInfo::from_kill
•	☑️ sys_sigaltstack câblé → thread.addresses.sigaltstack_base/size
•	☑️ sys_uname câblé → struct utsname 390 bytes ; "Exo-OS" x86_64
•	☑️ sys_execve argv/envp copiés depuis userspace (ARGV-01)
Phase 4 — ExoFS (obligatoire)
•	☐ Pipeline crypto : Blake3→compression→XChaCha20 dans cet ordre (CRYPTO-02)
•	☐ Nonce XChaCha20 : AtomicU64 + HKDF (jamais RDRAND seul) — ERR-08
•	☐ verify() constant-time (LAC-01) — pas de return early
•	☐ SYS_EXOFS_OPEN_BY_PATH=519 implémenté (BUG-01)
•	☐ __NR_getdents64=520 défini dans musl-exo (BUG-02)
•	☐ mount_secret_key initialisée depuis CSPRNG au montage — ERR-10
•	☐ EpochRecord checksum Blake3 vérifié au montage — ERR-12
•	☐ zstd-safe : shims ZSTD_malloc/ZSTD_free implémentés — ERR-09
•	☐ verify_cap() présent dans TOUS les handlers 500-519 (SYS-07)

Phase 5 — Servers Ring 1 (obligatoire)
•	☐ ipc_broker démarre en PID 2
•	☐ init démarre en PID 1 avec supervision SIGCHLD
•	☐ vfs_server monte ExoFS
•	☐ crypto_server est le SEUL service avec imports RustCrypto (SRV-04)

Phase 6 — Prérequis exo-boot lui-même (activer quand vert)
•	☐ kernel.elf compilé en ET_DYN (PIE) pour apply_pie_relocations()
•	☐ dual-entry detect_boot_path() implémenté dans early_init.rs
•	☐ BootInfo magic 0x4F42_5F53_4F4F_5845 vérifié en premier
•	☐ Test boot UEFI sur QEMU OVMF : exit=124 XK12356...ZAIOK
•	☐ kernel.elf sur partition ESP FAT32 (pas sur ExoFS)
