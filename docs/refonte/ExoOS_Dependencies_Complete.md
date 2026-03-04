EXO-OS
Référence Complète des Dépendances
Cargo.toml · Crates · Ce qui est Custom · Workspace corrigé
kernel (no_std)  ·  exofs (no_std)  ·  servers Ring 1 (std)  ·  exo-libc ecosystem Ring 3
📋  Méthodologie : chaque module de l'arborescence v6 croisé avec DOC1-10, ExoFS v3, doc_speciale v6, audits Gemini/Z-AI. Crates vérifiées no_std-compatibles. Ce qui manquait dans le Cargo.toml initial est identifié et corrigé.
 
1 — Vue d'ensemble : 4 couches de dépendances
Couche	Crate / Projet	no_std	std	Dépend de
kernel/	exo-os-kernel	✅ obligatoire	❌	rien (couche 0→3)
exofs/	exofs (crate séparée)	✅ obligatoire	❌	kernel (allocator, types)
servers/*	init, net_stack, crypto_server…	❌	✅	kernel via IPC+caps uniquement
exo-libc ecosystem	exo-rt, exo-libc, musl-exo	Ring 3	Ring 3	nos syscalls 0-519

⚠️  Le Cargo.toml workspace actuel a deux problèmes : (1) noms de servers incorrects par rapport à l'arborescence v6, (2) exofs/ manquant comme membre workspace. Voir Section 8 pour la version corrigée.

2 — kernel/Cargo.toml — Complet et annoté
🔴  Règle absolue : tout ce qui est dans kernel/ est no_std + alloc. Aucune crate qui importe std:: ou libc. Chaque crate listée ici a été vérifiée no_std-compatible.

[package]
name    = "exo-os-kernel"
version = "0.1.0"
edition = "2021"
 
[lib]
name       = "kernel"
crate-type = ["staticlib"]
 
[features]
default      = ["x86_64"]
x86_64       = []
aarch64      = []
kasan        = []              # memory/integrity/sanitizer.rs
proptest_int = ["proptest"]    # Tests proprietes INVARIANTS.md
 
[dependencies]
 
# ═══════════════════════════════════════════════════════════
# WORKSPACE HERITE
# ═══════════════════════════════════════════════════════════
spin = { workspace = true }
  # scheduler/sync/{spinlock,mutex,rwlock,condvar,barrier}.rs
  # memory/frame/pool.rs (EmergencyPool)
  # security/capability/ (spin::Once pour ELF_LOADER trait)
 
# ═══════════════════════════════════════════════════════════
# ARCHITECTURE  arch/x86_64/
# ═══════════════════════════════════════════════════════════
 
# arch/x86_64/boot/multiboot2.rs — parsing info struct Multiboot2
multiboot2 = { version = "3", default-features = false }
  # no_std natif — rust-osdev, concu pour OS dev
  # BootInformation, MemoryMapTag, ElfSectionsTag
 
# arch/x86_64/boot/uefi.rs — types UEFI (memory map)
uefi-raw = { version = "0.6", default-features = false }
  # Types uniquement — si exo-boot passe Multiboot2 sur UEFI, optionnel
 
# arch/x86_64/cpu/features.rs — CPUID (SSE/AVX/AES-NI/RDRAND/TSC)
raw-cpuid = { version = "11", default-features = false }
  # Wrapper securise instruction CPUID
  # Utilise aussi : arch/virt/detect.rs, security/exploit_mitigations/
 
# arch/x86_64/acpi/{parser,madt,hpet,pm_timer}.rs
acpi = { version = "5", default-features = false }
  # rust-osdev/acpi — concu pour kernels no_std
  # AcpiTables, Madt (CPU topology SMP), Hpet, PmTimer
 
# arch/x86_64/apic/ + memory/dma/ — registres MMIO
volatile = { version = "0.5", default-features = false }
  # Empeche optimisation compilateur sur acces MMIO
  # local_apic.rs (LAPIC), io_apic.rs, dma/channels/
 
# ═══════════════════════════════════════════════════════════
# STRUCTURES DE DONNEES KERNEL
# ═══════════════════════════════════════════════════════════
 
# Rights bitflags, MountFlags, PageFlags, FrameFlags, DmaFlags
bitflags = { version = "2", default-features = false }
  # Tous modules definissant des flags
  # security/capability/rights.rs, memory/frame/descriptor.rs
  # fs/core/vfs.rs, ipc/core/types.rs, process/resource/rlimit.rs
 
# security/capability/table.rs — CapTable par processus
# ipc/endpoint/registry.rs — registre endpoints
# exofs/cache/ + exofs/dedup/
hashbrown = { version = "0.14", default-features = false, features = ["alloc"] }
  # HashMap O(1) no_std+alloc — utiliser avec siphasher (keyed, anti-HashDoS)
  # PID allocator = IDR custom lock-free (pas hashbrown — voir Section 5)
 
# memory/virtual/vma/tree.rs — VMA red-black tree
# scheduler RunQueue (SCHED-08 : zero allocation dans ISR)
intrusive-collections = { version = "0.9", default-features = false }
  # RBTree intrusive — pas d allocation par noeud
 
# syscall/abi.rs (pt_regs), signal/frame.rs (SignalFrame),
# security/capability/token.rs (CapToken 128 bits), structs on-disk
zerocopy = { version = "0.7", default-features = false, features = ["derive"] }
  # Validation compile-time des layouts memoire
  # #[derive(FromBytes, AsBytes)] sur structs critiques
 
# ═══════════════════════════════════════════════════════════
# CRYPTOGRAPHIE — security/crypto/
# ═══════════════════════════════════════════════════════════
# REGLE : JAMAIS libsodium (requiert libc+std)
# JAMAIS ring crate (dependencies C complexes)
# JAMAIS implementation from scratch
# UNIQUEMENT RustCrypto no_std validees IETF
 
# security/crypto/blake3.rs + exofs/crypto/ (BlobId, checksums)
blake3 = { version = "1", default-features = false }
  # Partout : BlobId, superblock checksum, code signing (WORKSPACE)
 
# security/crypto/xchacha20_poly1305.rs + exofs/crypto/xchacha20.rs
chacha20poly1305 = { version = "0.10", default-features = false,
                     features = ["xchacha20"] }
  # feature xchacha20 OBLIGATOIRE — nonce 192 bits anti-reutilisation (WORKSPACE)
 
# security/crypto/aes_gcm.rs — IPC kernel-to-kernel chiffre
aes-gcm = { version = "0.10", default-features = false }
  # AES-NI si disponible (detecte via raw-cpuid)
 
# security/crypto/x25519.rs — echange cles (channels IPC securises)
x25519-dalek = { version = "2", default-features = false,
                 features = ["static_secrets"] }
 
# security/crypto/ed25519.rs + security/integrity_check/code_signing.rs
ed25519-dalek = { version = "2", default-features = false }
  # Verification signatures ELF + secure boot
 
# security/crypto/kdf.rs — derivation VolumeKey a ObjectKey
hkdf = { version = "0.12", default-features = false }
  # RFC 5869 (WORKSPACE)
 
# security/crypto/kdf.rs + exofs/crypto/master_key.rs — MasterKey depuis PIN
argon2 = { version = "0.5", default-features = false }
  # RFC 9106 — appele UNIQUEMENT au montage, jamais en hot path (WORKSPACE)
  # Parametres obligatoires dans key_storage.rs : time_cost, mem_cost, parallelism
 
# ═══════════════════════════════════════════════════════════
# HASH ANTI-HASHDOS
# ═══════════════════════════════════════════════════════════
 
# memory/utils/futex_table.rs + exofs/path/path_index.rs
# + hashbrown custom hasher (CapTable, endpoint registry)
siphasher = { version = "0.3", default-features = false }
  # SipHash-2-4 keyed — cle DOIT etre aleatoire via security::crypto::rng (WORKSPACE)
 
# ═══════════════════════════════════════════════════════════
# COMPRESSION
# ═══════════════════════════════════════════════════════════
# REGLE COMPRESS-MEM-01 : memory/swap/compress.rs = LZ4 UNIQUEMENT
# Zstd INTERDIT dans hot path swap (4KB pages, latence incompatible)
# LZ4 ~3 GB/s | Zstd niveau 1 ~400 MB/s
 
# memory/swap/compress.rs (zswap) + exofs/compress/lz4_wrapper.rs
lz4_flex = { version = "0.11", default-features = false }
  # Pure Rust no_std — zero dependance C (WORKSPACE)
 
# exofs/compress/zstd_wrapper.rs UNIQUEMENT (pas dans memory/)
zstd-safe = { version = "7", default-features = false, features = ["no_std"] }
  # Lie libzstd C — shims ZSTD_malloc/ZSTD_free requis (voir Section 4)
  # Alternative : ruzstd (decomp pure Rust) + encoder en Ring 1 via IPC
 
# ═══════════════════════════════════════════════════════════
# ELF PARSING
# ═══════════════════════════════════════════════════════════
# ARCHITECTURE : process/lifecycle/exec.rs definit TRAIT ElfLoader
# exofs/ implémente ce trait — process/ n'a pas de dep directe
 
xmas-elf = { version = "0.9", default-features = false }
  # exofs/posix_bridge/ + process/lifecycle/elf_loader.rs (impl trait)
  # Parsing PT_LOAD, PT_INTERP, PT_GNU_STACK, PT_GNU_RELRO, auxv
 
# ═══════════════════════════════════════════════════════════
# IPC — SERIALISATION
# ═══════════════════════════════════════════════════════════
# ipc/ring/spsc.rs + mpmc.rs = CUSTOM (CachePadded maison) — Section 5
 
postcard = { version = "1", default-features = false, features = ["alloc"] }
  # ipc/message/serializer.rs — format binaire compact messages IPC
 
# ═══════════════════════════════════════════════════════════
# LOGGING
# ═══════════════════════════════════════════════════════════
log = { version = "0.4", default-features = false }
  # Facade logging — impl custom vers ring buffer audit ou UART
 
[dev-dependencies]
proptest = { version = "1", default-features = false, features = ["alloc"] }
  # Tests proprietes INVARIANTS.md : caps delegation, lock ordering, BlobId
static_assertions = { version = "1" }
  # const_assert!(size_of::<EpochRecord>() == 104)
  # const_assert!(size_of::<CapToken>() == 16)

3 — exofs/Cargo.toml — Crate séparée no_std
ℹ️  exofs/ est une crate workspace séparée (elle a son propre lib.rs). Elle n'est pas encore dans le workspace Cargo.toml. À ajouter comme membre — voir Section 8.

[package]
name    = "exofs"
version = "0.1.0"
edition = "2021"
 
[features]
default      = ["zstd_kernel"]
zstd_kernel  = ["zstd-safe"]   # Compression Zstd Ring 0 (shims requis)
zstd_pure    = ["ruzstd"]      # Decomp Zstd pure Rust (sans shims C)
 
[dependencies]
spin             = { workspace = true }
bitflags         = { workspace = true }
zerocopy         = { workspace = true }
hashbrown        = { workspace = true }
siphasher        = { workspace = true }
lz4_flex         = { workspace = true }
blake3           = { workspace = true }
chacha20poly1305 = { workspace = true }
hkdf             = { workspace = true }
argon2           = { workspace = true }
log              = { workspace = true }
 
# Zstd — option kernel avec shims C
zstd-safe = { version = "7", default-features = false,
             features = ["no_std"], optional = true }
  # exofs/compress/zstd_wrapper.rs (CompressionAlgo::Zstd / ZstdMax)
 
# Zstd — alternative pure Rust (decomp uniquement)
ruzstd = { version = "0.7", default-features = false, optional = true }
  # Si zstd_kernel desactive : decomp lecture, compression via IPC Ring 1
 
# ELF parsing — implementation du trait ElfLoader (defini dans process/)
xmas-elf = { version = "0.9", default-features = false }
  # exofs/posix_bridge/vfs_compat.rs, process/lifecycle/ (impl trait)
 
[dev-dependencies]
proptest          = { workspace = true }
static_assertions = { workspace = true }

4 — Note critique : zstd-safe en Ring 0 (shims C requis)
🔴  zstd-safe lie la bibliothèque C libzstd. En Ring 0 il n'y a pas malloc/free. Ces fonctions DOIVENT être fournies — exactement comme Linux le fait dans lib/zstd/. Sans elles : undefined symbol à la compilation.

// exofs/compress/zstd_shims.rs — OBLIGATOIRE si feature zstd_kernel
 
#[no_mangle]
pub extern "C" fn ZSTD_malloc(size: usize,
    _opaque: *mut core::ffi::c_void) -> *mut core::ffi::c_void {
    let layout = alloc::alloc::Layout::from_size_align(size, 8).unwrap();
    unsafe { alloc::alloc::alloc(layout) as *mut core::ffi::c_void }
}
 
#[no_mangle]
pub extern "C" fn ZSTD_free(
    ptr: *mut core::ffi::c_void,
    _opaque: *mut core::ffi::c_void) {
    // WARN : Zstd ne passe pas la taille a free()
    // Utiliser un slab dedie Zstd plutot que buddy allocator
    if !ptr.is_null() { unsafe { /* slab_zstd_free(ptr) */ } }
}
 
#[no_mangle]
pub extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    unsafe { core::ptr::copy_nonoverlapping(src, dst, n); dst }
}
#[no_mangle]
pub extern "C" fn memset(s: *mut u8, c: i32, n: usize) -> *mut u8 {
    unsafe { core::ptr::write_bytes(s, c as u8, n); s }
}

🔧  Alternative recommandée si les shims sont jugés trop risqués : activer feature zstd_pure (ruzstd décompression pure Rust) et déléguer la compression Zstd au crypto_server Ring 1 via IPC. Seul LZ4 reste en Ring 0.

5 — Ce qui est 100% custom (aucune crate externe)
✅  Ces modules sont implémentés from scratch en Rust no_std. Ne pas tenter de les remplacer par des crates — elles n'existent pas en no_std adapté Ring 0, ou la conception maison est intentionnelle et documentée.

5.1 — signal/ et process/signal/ — la distinction kernel / Ring 3
📋  Distinction demandée explicitement : un module utilise redox-rt (Ring 3), l'autre est une implémentation complète Ring 0.

Module	Ring	Crates utilisées	Responsabilité
kernel/src/process/signal/ delivery.rs handler.rs mask.rs queue.rs default.rs	Ring 0 — kernel	spin (queue) zerocopy (SignalTcb layout) ASM pur (trampoline)	Livraison signal au retour kernel UNIQUEMENT Ne livre JAMAIS depuis le hot path scheduler Gestion masques + queue AtomicU64 Handlers defaut : SIGTERM→exit, SIGKILL→force
kernel/src/signal/ tcb.rs frame.rs delivery.rs trampoline.asm	Ring 0 — kernel	zerocopy (SignalFrame, SigactionEntry) spin (pending queue) ASM pur pour trampoline sigreturn	SignalTcb struct VALEUR (pas AtomicPtr — SIG-01) Construction frame sur stack Ring 3 Sauvegarde TOUS regs + FPU/SSE (SIG-11/12) Vérif magic 0x5349474E au sigreturn (SIG-13)
exo-rt (Ring 3) fork redox-os/redox-rt	Ring 3 — userspace	NOS syscalls uniquement (SYS_PROC_FORK, SYS_PROC_EXEC) Ecriture AtomicU64 → SignalTcb (0 syscall)	sigaction() → écriture directe TCB (0 syscall) sigprocmask() → AtomicU64 direct fork() : setup_child_stack + atfork handlers JAMAIS de code redox-rt dans le kernel

🔴  JAMAIS de code exo-rt ou redox-rt dans kernel/. exo-rt est Ring 3 uniquement. Le kernel implémente sa propre logique signal sans aucune libc.

5.2 — Tableau des modules custom
Module	Pourquoi custom	Crate externe rejetée et pourquoi
ipc/ring/spsc.rs + mpmc.rs (CachePadded maison)	head/tail sur cache lines SÉPARÉES (IPC-01) #[repr(C,align(64))] obligatoire Sans ça : false sharing → dégradation 10-100×	crossbeam-queue : std en pratique Pas de contrôle layout cache lines conc-queue : pas de CachePadded garanti
process/core/pid.rs (IDR radix tree lock-free)	PID allocator < 100 cycles Opération critique chemin critique fork() IDR = structure Linux O(1) lookup	Aucune crate no_std IDR disponible hashbrown OK pour CapTable mais trop lourd PIDs
scheduler/sync/ (WaitQueue, CondVar kernel-aware)	WaitQueue avec intrusive linked list CondVar doit connaitre le scheduler (SCHED-08) Barrier cross-CPU via IPI arch/	spin ne fournit pas WaitQueue CondVar std ne compile pas no_std Ring 0
scheduler/timer/ hrtimer tick clock deadline_timer	Lit TSC + HPET depuis arch/ Expire au tick scheduler APIC VDSO clock_gettime sans syscall	Aucune crate timer no_std pour kernel Dépend de arch/ (HPET, APIC timer) — crate impossible
scheduler/policies/ cfs rt deadline idle ai_guided	CFS = vruntime + RBTree (intrusive) AI = lookup table statique NUMA (ai_hints.rs) RT = politiques POSIX SCHED_FIFO/RR	Aucune crate ordonnancement kernel AI guided = table lookup maison, pas ML runtime
memory/heap/ buddy + slab + slub + global.rs	#[global_allocator] custom Buddy grandes allocs, Slab/SLUB < 4KB Magazine layer TLS pour < 25 cycles hot path	jemalloc, mimalloc : requièrent std+libc dlmalloc-rs : Ring 3 uniquement (exo-alloc Ring 3)
security/crypto/rng.rs	RDRAND + compteur atomique + mélange TSC Seed stockée en mémoire kernel protégée Aucune API OS disponible en Ring 0	getrandom : requiert syscall (Ring 3) rand : dépend de getrandom
arch/spectre/ smp/ virt/ (KPTI, retpoline, SSBD, IBRS…)	Assembleur pur : WRMSR, IBRS, STIBP, SSBD CPUID → MSR writes SMEP/SMAP : bit dans CR4/CR0	Aucune crate — opérations x86_64 Ring 0 brutes raw-cpuid seulement pour détection features
ipc/core/fastcall_asm.s (fast IPC path)	< 50 cycles objectif Évite le trampoline SYSCALL complet Optimisation assembly manuelle ExoOS	Aucune crate — IPC path spécifique ExoOS Dépend du protocole fastcall ExoOS
fs/io/uring.rs (io_uring kernel side)	Implémentation côté kernel de io_uring Gestion SQE/CQE ring buffers Intégration avec notre VFS	io-uring crate : côté userspace uniquement Kernel side = toujours custom

6 — servers/*/Cargo.toml — Ring 1 (std disponible)
✅  Les servers Ring 1 utilisent std. Ils communiquent avec le kernel UNIQUEMENT via IPC+capabilities. Accès direct aux structures kernel : INTERDIT.

servers/net_stack/ — Stack TCP/IP
[dependencies]
smoltcp = { version = "0.11", features = [
    "socket-tcp", "socket-udp", "socket-raw",
    "proto-ipv4", "proto-ipv6", "proto-dhcpv4", "async"
]}
  # TCP/IP complet : state machine TCP RFC 9293, IPv4/v6, ARP, ICMP
  # Peut etre no_std mais utilise std ici pour simplicite Ring 1
log = "0.4"

servers/crypto_server/ — Cryptographie (SRV-04 : seul service autorise)
⚠️  SRV-04 : crypto_server est le SEUL service autorisé à faire de la crypto. Tous les autres servers délèguent ici. Ne jamais dupliquer ces dépendances ailleurs.
[dependencies]
rustls    = { version = "0.23", features = ["std"] }
  # TLS 1.3 pure Rust audite — tls.rs
x509-cert = { version = "0.2", features = ["std", "builder"] }
der       = { version = "0.7", features = ["std"] }
  # Certificats X.509 — RustCrypto — certs.rs
aes-gcm   = "0.10"
  # Keystore chiffrement AES-256-GCM — keystore.rs
blake3    = "1"
hkdf      = "0.12"
ed25519-dalek = "2"
x25519-dalek  = "2"
getrandom = { version = "0.2", features = ["rdrand"] }
  # entropy service Ring 3 — rng_service.rs
log = "0.4"

servers/login_manager/ — Authentification
[dependencies]
# Hachage mots de passe (pam.rs) — le doc dit SHA3-512 + scrypt
# Recommandation : migrer vers Argon2id (RFC 9106, plus moderne)
argon2 = "0.5"      # Argon2id recommande
scrypt = "0.11"     # retrocompatibilite
sha3   = "0.10"     # SHA3-512 derivations annexes
rand   = { version = "0.8", features = ["getrandom"] }
uuid   = { version = "1", features = ["v4"] }
log = "0.4"

servers/network_manager/ + servers/power_manager/
# network_manager/Cargo.toml
[dependencies]
trust-dns-resolver = "0.23"   # DNS + DNS-over-TLS
log = "0.4"
 
# power_manager/Cargo.toml
[dependencies]
acpi = "5"   # version std — meme crate que kernel avec std features
log = "0.4"

servers/init/ + servers/ipc_broker/ + servers/shield/ + servers/vfs_server/
✅  Ces servers sont 100% custom. Pas de dépendances complexes.
# Commun a tous ces servers
[dependencies]
log        = "0.4"
env_logger = "0.11"   # dev uniquement
# Types partages kernel -> servers (CapToken, Rights, ObjectId)
# A exporter depuis une crate separee : libs/exo-types/ (no_std+std)
exo-os-types = { path = "../../libs/types" }

7 — Écosystème exo-libc (Ring 3 — hors kernel workspace)
📋  Ces projets compilent séparément et s'installent dans le sysroot de la toolchain ExoOS. Pas dans le workspace kernel.

Dépôt	Fork de	Travail requis	Phase	Note critique
exo-os/musl-exo	musl 1.2.5 (C)	Modifier syscall_arch.h uniquement __NR_open → SYS_EXOFS_OPEN_BY_PATH=519 __NR_getdents64 → SYS_EXOFS_READDIR=520	Phase 1	Ne JAMAIS toucher les signal handlers C musl (matures) LIB-01 + LIB-02
exo-os/exo-rt	redox-os/redox-rt	Remplacer appels libredox par nos syscalls SYS_PROC_FORK + setup_child_stack Ecriture AtomicU64 SignalTcb (0 syscall)	Phase 2	JAMAIS de dépendance libredox Zero code Ring 0 dans exo-rt
exo-os/exo-libc	redox-os/relibc	Ecrire src/platform/exo_os/mod.rs (~2000 SLoC) impl Pal for ExoOs {} Pal::open() = 2 syscalls (RESOLVE+OPEN)	Phase 2	JAMAIS exposer ObjectId a l'app POSIX (LIB-05) cbindgen genere les headers C auto
exo-os/exo-libm	openlibm (C+Rust)	Aucune modification Drop-in complet	Phase 1	relibc et musl l'utilisent deja comme backend libm
exo-os/exo-alloc	dlmalloc-rs	Configurer backend mmap() → SYS_MEM_MAP syscall 9	Phase 2	Heap allocateur userspace Ring 3
exo-os/exo-toolchain	rustc + libstd	target x86_64-unknown-exo dans rustc_target libstd fork : sys/unix → sys/exo_os libc = exo-libc dans Cargo.toml target	Phase 3	Rust std natif pour programmes ExoOS

8 — Workspace Cargo.toml — Version corrigée
🐛  Deux problèmes dans le Cargo.toml actuel : (1) noms de servers incorrects vs arborescence v6, (2) exofs/ et plusieurs servers manquants comme membres.

# Cargo.toml — WORKSPACE ROOT CORRIGE
[workspace]
resolver = "2"
members = [
    "kernel",
    "exofs",                     # AJOUT — crate separee no_std
 
    # Servers — noms corriges pour correspondre a l'arborescence v6
    "servers/init",              # etait init_server
    "servers/shield",            # etait exo_shield
    "servers/net_stack",         # AJOUT — stack TCP/IP Ring 1 isolee
    "servers/network_manager",   # etait network_server
    "servers/crypto_server",     # inchange
    "servers/ipc_broker",        # etait ipc_router
    "servers/vfs_server",        # AJOUT
    "servers/power_manager",     # AJOUT
    "servers/login_manager",     # AJOUT
    "servers/device_server",     # inchange
    "servers/memory_server",     # a evaluer — peut fusionner avec kernel
    "servers/scheduler_server",  # a evaluer — peut fusionner avec kernel
]
exclude = ["libs", "exo-rt", "exo-libc", "musl-exo", "exo-alloc"]
 
[workspace.package]
version = "0.1.0"
edition = "2021"
authors = ["Exo-OS Team"]
license = "MIT OR Apache-2.0"
 
# ─────────────────────────────────────────────────────────
# Dependances PARTAGEES — version unique dans tout le workspace
# ─────────────────────────────────────────────────────────
[workspace.dependencies]
 
# Sync kernel
spin = { version = "0.9", default-features = false,
         features = ["spin_mutex", "rwlock", "once", "barrier"] }
 
# Structures de donnees
bitflags              = { version = "2",    default-features = false }
zerocopy              = { version = "0.7",  default-features = false, features = ["derive"] }
hashbrown             = { version = "0.14", default-features = false, features = ["alloc"] }
intrusive-collections = { version = "0.9",  default-features = false }
 
# Architecture
multiboot2 = { version = "3",   default-features = false }
raw-cpuid  = { version = "11",  default-features = false }
acpi       = { version = "5",   default-features = false }
volatile   = { version = "0.5", default-features = false }
 
# Cryptographie (partagee kernel + exofs, versionnee une seule fois)
blake3           = { version = "1",    default-features = false }
chacha20poly1305 = { version = "0.10", default-features = false, features = ["xchacha20"] }
aes-gcm          = { version = "0.10", default-features = false }
x25519-dalek     = { version = "2",    default-features = false, features = ["static_secrets"] }
ed25519-dalek    = { version = "2",    default-features = false }
hkdf             = { version = "0.12", default-features = false }
argon2           = { version = "0.5",  default-features = false }
siphasher        = { version = "0.3",  default-features = false }
 
# Compression
lz4_flex = { version = "0.11", default-features = false }
 
# ELF
xmas-elf = { version = "0.9", default-features = false }
 
# IPC
postcard = { version = "1", default-features = false, features = ["alloc"] }
 
# Logging
log = { version = "0.4", default-features = false }
 
# Tests
proptest          = { version = "1", default-features = false, features = ["alloc"] }
static_assertions = { version = "1" }
 
[profile.dev]
opt-level = 0
debug     = true
panic     = "abort"
 
[profile.release]
opt-level     = 3
lto           = true
panic         = "abort"
codegen-units = 1
strip         = "none"

9 — Récapitulatif : toutes les crates du projet
9.1 — kernel + exofs (no_std) — 25 crates
Crate	Version	no_std	Modules principaux	Statut
spin	0.9	✅	scheduler/sync/, memory/pool.rs, capability/Once	Workspace — EXISTAIT
multiboot2	3	✅	arch/x86_64/boot/multiboot2.rs	MANQUAIT
uefi-raw	0.6	✅	arch/x86_64/boot/uefi.rs	MANQUAIT — optionnel
raw-cpuid	11	✅	arch/cpu/features.rs, virt/detect.rs	MANQUAIT
acpi	5	✅	arch/acpi/{parser,madt,hpet,pm_timer}	MANQUAIT
volatile	0.5	✅	arch/apic/, memory/dma/ (MMIO)	MANQUAIT
bitflags	2	✅	Rights, MountFlags, PageFlags, FrameFlags…	MANQUAIT — Workspace
hashbrown	0.14	✅	capability/table.rs, endpoint/registry.rs, exofs/cache/	MANQUAIT — Workspace
intrusive-collections	0.9	✅	vma/tree.rs (RBTree), scheduler RunQueue	MANQUAIT
zerocopy	0.7	✅	pt_regs, SignalFrame, CapToken, structs on-disk	MANQUAIT — Workspace
blake3	1	✅	security/crypto/blake3.rs, exofs/crypto/ (BlobId)	MANQUAIT — Workspace
chacha20poly1305	0.10	✅	security/crypto/xchacha20.rs, exofs/crypto/	MANQUAIT — feature xchacha20 — Workspace
aes-gcm	0.10	✅	security/crypto/aes_gcm.rs (IPC chiffré)	MANQUAIT
x25519-dalek	2	✅	security/crypto/x25519.rs	MANQUAIT — feature static_secrets
ed25519-dalek	2	✅	security/crypto/ed25519.rs, code_signing.rs	MANQUAIT
hkdf	0.12	✅	security/crypto/kdf.rs, exofs/crypto/key_derivation	MANQUAIT — RFC 5869 — Workspace
argon2	0.5	✅	security/crypto/kdf.rs, exofs/crypto/master_key	MANQUAIT — RFC 9106 — Workspace
siphasher	0.3	✅	futex_table.rs, exofs/path/path_index.rs	MANQUAIT — Workspace
lz4_flex	0.11	✅	memory/swap/compress.rs, exofs/compress/lz4	MANQUAIT — Workspace
zstd-safe	7	⚠️	exofs/compress/zstd_wrapper.rs UNIQUEMENT	MANQUAIT — shims C requis
ruzstd	0.7	✅	exofs/compress/ (alt Zstd décomp pure Rust)	MANQUAIT — optionnel
xmas-elf	0.9	✅	exofs/posix_bridge/ (impl trait ElfLoader)	MANQUAIT
postcard	1	✅	ipc/message/serializer.rs	MANQUAIT
log	0.4	✅	Tous modules	MANQUAIT — Workspace
proptest + static_assertions	1	✅ dev	Tests INVARIANTS.md	MANQUAIT — dev-dep — Workspace

9.2 — servers Ring 1 (std) — 14 crates
Crate	Version	Server(s)	Note
smoltcp	0.11	net_stack/	TCP/IP complet RFC 9293
rustls	0.23	crypto_server/	TLS 1.3 — SRV-04
x509-cert	0.2	crypto_server/	Certificats X.509 — RustCrypto
der	0.7	crypto_server/	ASN.1 DER parsing
getrandom	0.2	crypto_server/	feature rdrand
aes-gcm	0.10	crypto_server/	std features — keystore
blake3	1	crypto_server/	std features
ed25519-dalek	2	crypto_server/	std features
x25519-dalek	2	crypto_server/	std features
argon2 (std)	0.5	login_manager/	Argon2id mots de passe
scrypt	0.11	login_manager/	Rétrocompatibilité
sha3	0.10	login_manager/	SHA3-512
trust-dns-resolver	0.23	network_manager/	DNS + DoT
acpi (std)	5	power_manager/	std features — même crate kernel

9.3 — exo-libc ecosystem Ring 3
Projet	Fork	Phase	Dépendances propres
musl-exo	musl 1.2.5 (C)	1	Aucune crate Rust — modification syscall_arch.h uniquement
exo-rt	redox-os/redox-rt	2	Nos syscalls uniquement — zéro libredox
exo-libc	redox-os/relibc	2	exo-rt + dlmalloc-rs + cbindgen (dev)
exo-libm	openlibm	1	Aucune modification
exo-alloc	dlmalloc-rs	2	Backend SYS_MEM_MAP
exo-toolchain	rustc + libstd	3	target x86_64-unknown-exo + std::sys::exo_os

