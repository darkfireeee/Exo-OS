EXO-OS · ExoFS v3 — Module Syscall
Analyse Complète & Intégration Kernel
ABI · Dispatch · Handlers · ExoFS · Libc Redox-inspirée · Sécurité
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Document basé sur l'implémentation réelle du kernel (kernel/src/syscall/)
Adapté des libs Redox OS (redox-rt, relibc) pour Exo-OS
Date : 2026-03-04 · Version : v4.0 — remplace ExoFS_Syscall_Analyse v3

1 — Architecture de la Couche Syscall
La couche syscall Exo-OS est structurée en trois niveaux distincts. Cette séparation est non-négociable.

1.1 — Vue d'ensemble
kernel/src/
├── syscall/
│   ├── mod.rs          # register_all_syscalls(), dispatch entry point
│   ├── numbers.rs      # SYS_READ=0 … SYS_EXOFS_EPOCH_COMMIT=518
│   │                   # Constantes PUBLIQUES — utilisées par exo-libc
│   ├── table.rs        # static SYSCALL_TABLE: [SyscallFn; 520]
│   │                   # bounds check AVANT dispatch (SYS-02)
│   ├── abi.rs          # Convention registres x86_64
│   ├── trampoline.asm  # Entrée Ring3→Ring0 : SWAPGS, save rsp, pt_regs
│   ├── errno.rs        # KernelError → -errno POSIX
│   └── handlers/       # THIN WRAPPERS UNIQUEMENT — zéro logique métier
│       ├── process.rs  # fork/exec/exit/wait → délègue process::
│       ├── signal.rs   # sigaction/kill/sigreturn → délègue signal::
│       ├── fd.rs       # read/write/open/close/dup → délègue fd::
│       ├── fs_posix.rs # stat/mkdir/unlink → délègue fs/exofs/syscall/
│       ├── memory.rs   # mmap/munmap/mprotect/brk → délègue memory::
│       ├── time.rs     # clock_gettime/nanosleep → délègue time::
│       └── misc.rs     # getuid/getgid/uname/sysinfo
 
└── fs/exofs/syscall/   # Logique ExoFS — appelée PAR handlers/fs_posix.rs
    ├── mod.rs          # register_exofs_syscalls() → table kernel 500-518
    ├── path_resolve.rs # SYS_EXOFS_PATH_RESOLVE (500)
    ├── object_open.rs  # SYS_EXOFS_OBJECT_OPEN (501)
    ├── object_read.rs  # SYS_EXOFS_OBJECT_READ (502)
    ├── object_write.rs # SYS_EXOFS_OBJECT_WRITE (503)
    ├── object_create.rs# SYS_EXOFS_OBJECT_CREATE (504)
    ├── object_delete.rs# SYS_EXOFS_OBJECT_DELETE (505)
    ├── object_stat.rs  # SYS_EXOFS_OBJECT_STAT (506)
    ├── object_set_meta.rs # SYS_EXOFS_OBJECT_SET_META (507)
    ├── get_content_hash.rs# SYS_EXOFS_GET_CONTENT_HASH (508) — audité SEC-09
    ├── snapshot_create.rs # SYS_EXOFS_SNAPSHOT_CREATE (509)
    ├── snapshot_list.rs   # SYS_EXOFS_SNAPSHOT_LIST (510)
    ├── snapshot_mount.rs  # SYS_EXOFS_SNAPSHOT_MOUNT (511)
    ├── relation_create.rs # SYS_EXOFS_RELATION_CREATE (512)
    ├── relation_query.rs  # SYS_EXOFS_RELATION_QUERY (513)
    ├── gc_trigger.rs      # SYS_EXOFS_GC_TRIGGER (514)
    ├── quota_query.rs     # SYS_EXOFS_QUOTA_QUERY (515)
    ├── export_object.rs   # SYS_EXOFS_EXPORT_OBJECT (516)
    ├── import_object.rs   # SYS_EXOFS_IMPORT_OBJECT (517)
    ├── epoch_commit.rs    # SYS_EXOFS_EPOCH_COMMIT (518)
    └── validation.rs      # copy_from_user() helpers — utilisé par TOUS

1.2 — Table des numéros syscall (syscall/numbers.rs)
Numéro	Constante	Destination	Notes
0	SYS_READ	handlers/fd.rs	POSIX — inchangé
1	SYS_WRITE	handlers/fd.rs	POSIX — inchangé
3	SYS_CLOSE	handlers/fd.rs	POSIX — inchangé
5	SYS_FSTAT	handlers/fd.rs	POSIX — inchangé
9	SYS_MMAP	handlers/memory.rs	POSIX — inchangé
57	SYS_FORK	handlers/process.rs	POSIX — kernel fait CoW
59	SYS_EXECVE	handlers/process.rs	POSIX — via ElfLoader
60	SYS_EXIT	handlers/process.rs	POSIX — inchangé
500	SYS_EXOFS_PATH_RESOLVE	fs/exofs/syscall/path_resolve.rs	ExoFS : path→ObjectId
501	SYS_EXOFS_OBJECT_OPEN	fs/exofs/syscall/object_open.rs	ExoFS : oid+rights→fd
502	SYS_EXOFS_OBJECT_READ	fs/exofs/syscall/object_read.rs	ExoFS
503	SYS_EXOFS_OBJECT_WRITE	fs/exofs/syscall/object_write.rs	ExoFS
504	SYS_EXOFS_OBJECT_CREATE	fs/exofs/syscall/object_create.rs	ExoFS
505	SYS_EXOFS_OBJECT_DELETE	fs/exofs/syscall/object_delete.rs	ExoFS
506	SYS_EXOFS_OBJECT_STAT	fs/exofs/syscall/object_stat.rs	ExoFS
507	SYS_EXOFS_OBJECT_SET_META	fs/exofs/syscall/object_set_meta.rs	ExoFS
508	SYS_EXOFS_GET_CONTENT_HASH	fs/exofs/syscall/get_content_hash.rs	ExoFS — audité
509–517	SYS_EXOFS_SNAPSHOT_*…IMPORT	fs/exofs/syscall/…	ExoFS
518	SYS_EXOFS_EPOCH_COMMIT	fs/exofs/syscall/epoch_commit.rs	ExoFS — 3 barrières NVMe
519–?	(réservé)	→ ENOSYS	Non documenté — voir BUG-02

2 — ABI x86_64 — Convention d'appel
2.1 — Registres
✅ ABI-01 — Registres d'entrée : rax=numéro, rdi=arg1, rsi=arg2, rdx=arg3, r10=arg4, r8=arg5, r9=arg6
✅ ABI-02 — Retour : rax ≥ 0 = succès, rax < 0 = -errno. Ex : ENOENT(2) → rax = -2 = 0xFFFFFFFFFFFFFFFE
🔴 ABI-03 — INTERDIT : retourner un pointeur kernel ou une enum brute dans rax → info-leak KASLR
🔴 ABI-04 — INTERDIT : modifier rdi/rsi/rdx dans le handler — le trampoline les a sauvés sur pt_regs
✅ ABI-05 — Stack kernel alignée 16 bytes à l'entrée du handler — vérifier avant appel C
✅ ABI-06 — SWAPGS obligatoire à l'entrée ET à la sortie du trampoline (x86_64 KPTI)
🔴 ABI-07 — INTERDIT : utiliser l'instruction SYSCALL depuis Ring 0 — appel direct de fonction
✅ ABI-08 — pt_regs complet sauvegardé sur la kernel stack — accessible pour ptrace

2.2 — Trampoline assembleur (syscall/trampoline.asm)
syscall_entry:
    swapgs                          # Échanger GS : user GS ↔ kernel GS
    mov [gs:CPU_RSP_SAVE], rsp      # Sauvegarder rsp Ring3
    mov rsp, [gs:CPU_KERNEL_RSP]    # Charger rsp kernel
    
    # Construire pt_regs complet sur la kernel stack
    push rcx        # rip Ring3 (SYSCALL écrase rcx avec RIP)
    push r11        # rflags Ring3 (SYSCALL écrase r11 avec RFLAGS)
    push rax        # numéro syscall
    push rdi        # arg1
    push rsi        # arg2
    push rdx        # arg3
    push r10        # arg4 (r10, PAS rcx — SYSCALL a écrasé rcx)
    push r8         # arg5
    push r9         # arg6
    push rbx; push rbp; push r12; push r13; push r14; push r15
    
    mov rdi, rsp    # &pt_regs comme argument
    call syscall_dispatch
    
    # Restaurer registres callee-saved
    pop r15; pop r14; pop r13; pop r12; pop rbp; pop rbx
    pop r9; pop r8; pop r10; pop rdx; pop rsi; pop rdi
    pop rax         # numéro (écrasé par valeur de retour)
    # rax = valeur de retour placée par syscall_dispatch
    pop r11         # restaurer rflags
    pop rcx         # restaurer rip Ring3
    
    # ATTENTION : vérifier RCX canonique avant SYSRETQ — errata Intel/AMD
    # Si non-canonique → SIGSEGV au processus, pas fault Ring 0
    call verify_rcx_canonical_or_sigsegv
    swapgs
    sysretq

2.3 — Dispatch et sécurité
🔴 SYS-01 — copy_from_user() OBLIGATOIRE pour TOUT pointeur Ring3→Ring0 dans les handlers
🔴 SYS-02 — Vérifier bounds du numéro AVANT dispatch : if rax >= TABLE_SIZE → -ENOSYS
🔴 SYS-03 — INTERDIT : logique métier dans handlers/ — thin wrappers uniquement
🔴 SYS-04 — INTERDIT : accéder à un pointeur userspace sans copy_from_user() — exploit garanti
✅ SYS-05 — Valider longueurs AVANT copy_from_user : len=0 → -EINVAL, len>MAX → -E2BIG
🔴 SYS-06 — INTERDIT : retourner une adresse kernel dans rax — info-leak KASLR
🔴 SYS-07 — verify_cap() appelé dans le handler AVANT de déléguer à la logique métier
🔴 SYS-09 — INTERDIT : syscall bloquant sans relâcher les locks tenus — deadlock kernel
✅ SYS-10 — Syscalls 0-499 = POSIX standard. Syscalls 500-518 = ExoFS natif
🔴 SYS-11 — INTERDIT : modifier SYSCALL_TABLE au runtime — statique, initialisée au boot

Exemple thin wrapper correct
// ✅ CORRECT — handlers/fd.rs : thin wrapper
pub fn sys_read_handler(args: &SyscallArgs) -> SyscallResult {
    let fd  = args.arg0 as i32;
    let ptr = args.arg1 as *mut u8;
    let len = args.arg2;
    
    if len > MAX_RW_SIZE { return Err(KernelError::InvalidArg); }
    
    // OBLIGATOIRE : copy_from_user pour le buffer
    let buf = unsafe { copy_from_user_slice(ptr, len)? };
    
    fd::io::do_read(fd, buf)  // délègue → zéro logique ici
}
 
// Convention retour : rax = -(errno as i64) as u64 pour les erreurs
// ex : ENOENT (2) → rax = 0xFFFFFFFFFFFFFFFE

3 — Mapping errno (syscall/errno.rs)
3.1 — Règles
🔴 ERRNO-01 — Toujours rax = -(errno as i64) as u64. Jamais errno positif dans rax en cas d'erreur.
🔴 ERRNO-02 — INTERDIT : retourner -1 sans fixer l'errno → app voit -1 mais errno=0 = comportement POSIX indéfini
✅ ERRNO-03 — syscall/errno.rs DOIT couvrir TOUS les variants de KernelError et ExofsError

3.2 — Table de mapping complète
Erreur Kernel	errno POSIX	Code numérique	Notes
KernelError::NotFound / ExofsError::NotFound	-ENOENT	2	Objet ou chemin absent
KernelError::NoMemory / ExofsError::NoMemory	-ENOMEM	12	OOM — jamais panic
KernelError::InvalidArg	-EINVAL	22	len=0, ptr null, flags invalides
KernelError::NoSpace / ExofsError::NoSpace	-ENOSPC	28	Espace disque épuisé
CapError::Denied	-EACCES	13	verify_cap() retourne Denied
CapError::ObjectNotFound	-ENOENT	2	Même traitement que NotFound
ExofsError::Corrupt	-EIO	5	Corruption détectée (magic, checksum)
KernelError::NotExecutable	-ENOEXEC	8	ObjectKind != Code
KernelError::TimedOut	-ETIMEDOUT	110	Attente expirée
KernelError::WouldBlock	-EAGAIN	11	O_NONBLOCK + pas de données
KernelError::TooBig	-E2BIG	7	len > MAX
KernelError::Interrupted	-EINTR	4	Signal reçu pendant sleep
KernelError::NotSupported	-ENOSYS	38	Syscall non implémenté
ExofsError::QuotaExceeded	-EDQUOT	122	Quota capability dépassé
ExofsError::VersionMismatch	-EPROTO	71	Format disque incompatible

🐛 BUG ERRNO-MISSING — errno.rs dans ExoFS v3 ne liste que 3 cas (NoSpace→ENOSPC, NotFound→ENOENT, Denied→EACCES). Les 12 autres variants de KernelError/ExofsError ne sont pas mappés. Sans mapping complet, le kernel retourne -1 avec errno=0 ou une valeur aléatoire pour ces cas.

4 — Syscalls Process : fork, exec, signal
4.1 — SignalTcb — structure partagée kernel/userspace
🔴 SIG-01 — INTERDIT : AtomicPtr<sigaction> dans SignalTcb — exploitation TOCTOU triviale (kernel déréférence pointeur userspace modifiable entre lecture et deref)
🔴 SIG-02 — SigactionEntry stocke les valeurs directement — le kernel SAUTE à handler_vaddr, ne le déréférence JAMAIS
🔴 SIG-03 — INTERDIT : adresse TCB fixe dans l'espace d'adressage — passer via AT_SIGNAL_TCB dans auxv (ASLR safe)
🔴 SIG-07 — SIGKILL et SIGSTOP non-maskables — ignorés dans SignalTcb.blocked
🔴 SIG-13 — magic 0x5349474E dans le frame — vérifié au sigreturn AVANT tout restore
🔴 SIG-14 — INTERDIT : sigreturn sans vérifier magic — injection de faux contexte Ring3
🔴 SIG-18 — INTERDIT : kernel écrit signal frame sans copy_to_user() — même si page partagée

Structure SignalTcb (correcte — v3)
// kernel/src/signal/tcb.rs
#[repr(C, align(64))]
pub struct SignalTcb {
    pub blocked:        AtomicU64,           // sigprocmask
    pub pending:        AtomicU64,           // signaux en attente
    pub handlers:       [SigactionEntry; 64],// VALEUR, pas AtomicPtr
    pub in_handler:     AtomicU32,           // compteur réentrance
    pub altstack_sp:    AtomicU64,           // SA_ONSTACK
    pub altstack_size:  AtomicU64,
    pub altstack_flags: AtomicU32,
    _pad:               [u8; 4],
}
 
#[repr(C)]
pub struct SigactionEntry {
    pub handler_vaddr: u64,  // adresse Ring3 — kernel SAUTE ici, ne déréférence PAS
    pub flags:         u32,  // SA_RESTART | SA_SIGINFO | SA_NODEFER | SA_ONSTACK
    pub mask:          u64,  // signaux bloqués pendant ce handler
    pub restorer:      u64,  // adresse trampoline sigreturn Ring3
}
// Adresse passée via auxv[AT_SIGNAL_TCB] — jamais adresse fixe (ASLR)

4.2 — fork() hybride
🔴 FORK-01 — INTERDIT : CoW depuis Ring1 — race TLB garantie, page_table_lock non tenable
🔴 FORK-02 — CoW : tenir page_table_lock PENDANT mark_all_pages_cow + tlb_shootdown
🔴 FORK-03 — TLB shootdown IPI à TOUS les CPUs actifs du processus (pas seulement current)
✅ FORK-05 — Child : SignalTcb.pending = 0 — pas d'héritage des signaux en attente
✅ FORK-08 — Cap table fork : caps FD héritées, caps mémoire CoW, caps IPC clonées
🔴 FORK-09 — INTERDIT : flush write buffers userspace dans le kernel — c'est exo-libc qui fait fflush() avant fork()
ℹ️ FORK-11 — fork() hybride : kernel CoW+PCB+caps, exo-rt setup_child_stack+TCB+atfork handlers

4.3 — exec() — étapes obligatoires
pub fn do_exec(proc: &mut Process, args: &ExecArgs) -> Result<!, KernelError> {
    // 1. copy_from_user : path, argv, envp (SYS-01 — obligatoire)
    let path = copy_path_from_user(args.path_ptr, args.path_len)?;
    let argv = copy_argv_from_user(args.argv_ptr)?;
    let envp = copy_envp_from_user(args.envp_ptr)?;
 
    // 2. Résoudre + vérifier ObjectKind::Code
    let bin_oid = exofs::path_resolve(&path)?;
    let cap = proc.cap_table.find_for_object(bin_oid)?;
    cap.check_right(Rights::EXEC)?;
    if obj.kind != ObjectKind::Code { return Err(KernelError::NotExecutable); }
 
    // 3. Bloquer signaux pendant exec (PROC-03 — éviter livraison sur ancien handler)
    proc.signal_tcb.block_all_except_kill();
 
    // 4. Valider et charger ELF (EXEC-11 : vérifier magic 0x7F ELF en premier)
    let elf = elf_loader::validate_and_load(obj)?;
 
    // 5. FD_CLOEXEC : révoquer caps O_CLOEXEC (EXEC-04 — POSIX obligatoire)
    proc.cap_table.revoke_cloexec();
 
    // 6. Appliquer ExecCapPolicy (Inherit/Revoke/Ambient)
    proc.cap_table.apply_exec_policy(&args.cap_policy)?;
 
    // 7. Réinitialiser SignalTcb (EXEC-05 : handlers → SIG_DFL, sigmask héritée)
    proc.signal_tcb.reset_for_exec();
 
    // 8. Setup stack + auxv
    let stack_top = setup_initial_stack(&argv, &envp, &elf, proc)?;
    push_auxv(stack_top, &elf, proc.signal_tcb_vaddr, VDSO_VADDR)?;
 
    // 9. ★ MANQUANT dans v3 : initialiser %fs pour TLS (PROC-10)
    arch::set_fs_base(proc.tls_initial_addr)?;
 
    // 10. Sauter au point d'entrée — jamais de retour
    jump_to_entry(elf.entry_point, stack_top)
}
✅ EXEC-01 — copy_from_user() pour path, argv, envp AVANT toute utilisation
✅ EXEC-02 — Vérifier ObjectKind::Code — Blob/Secret non exécutables
✅ EXEC-04 — FD_CLOEXEC révoqué automatiquement à exec() — POSIX obligatoire
🔴 EXEC-07 — Stack initiale alignée 16 bytes (rsp % 16 == 0) avant le call entry
🔴 EXEC-08 — Auxiliary vector : AT_PHDR, AT_PHNUM, AT_ENTRY, AT_RANDOM, AT_SIGNAL_TCB, AT_SYSINFO_EHDR

5 — Auxiliary Vector (process/auxv.rs)
L'auxv est le seul canal du kernel vers la libc au démarrage. Incomplet = exo-libc ne peut pas s'initialiser.

Constante AT_*	Valeur	Description	Obligatoire
AT_NULL	0	Terminateur — DOIT être en dernier	✅ OUI
AT_PHDR	3	Adresse des program headers ELF	✅ OUI
AT_PHNUM	5	Nombre de program headers	✅ OUI
AT_PAGESZ	6	Taille de page (4096)	✅ OUI
AT_ENTRY	9	Entry point ELF	✅ OUI
AT_UID	11	UID (0 pour l'instant)	✅ OUI
AT_GID	13	GID	✅ OUI
AT_RANDOM	25	16 bytes aléatoires RDRAND — stack canary + ASLR userspace	✅ OUI
AT_SYSINFO_EHDR	33	Adresse VDSO (ELF header) — clock_gettime sans syscall	✅ OUI
AT_SIGNAL_TCB	51	Adresse du SignalTcb — custom ExoOS (jamais adresse fixe)	✅ OUI
AT_CAP_TOKEN	52	CapToken initial du processus — custom ExoOS	⚠️ Voir note

ℹ️ AUXV-NOTE — AT_CAP_TOKEN(52) : le CapToken initial est passé via l'auxv pour que exo-libc puisse initialiser sa première opération. Ce token est visible par le processus (l'auxv est lisible en Ring3). C'est voulu : le processus possède ce token. Mais un fork() duplique la cap table — l'enfant n'a pas besoin de lire l'auxv pour obtenir ses caps. L'utilité de AT_CAP_TOKEN est donc limitée à l'initialisation de exo-libc. Les caps créées après exec() ne passent jamais par l'auxv.

✅ VDSO-01 — clock_gettime(CLOCK_MONOTONIC) via VDSO — lecture TSC directe, 10× plus rapide qu'un syscall
🔴 VDSO-03 — INTERDIT : données mutables dans le VDSO — read-only Ring3, kernel écrit via mapping séparé
✅ VDSO-05 — seqlock dans les données VDSO — Ring3 lit une version paire (écriture complète garantie)

6 — Compatibilité POSIX : musl, exo-rt, exo-libc
6.1 — Architecture des couches
Applications POSIX (bash, gcc, Python, programmes C/Rust)
     │ appelle fonctions C standard
     ▼
exo-libc (Ring3 — fork de relibc)
     │ impl Pal for ExoOs { open()→PATH_RESOLVE+OBJECT_OPEN … }
     │ traduit API POSIX → nos syscalls
     ▼
Syscalls Exo-OS (0-518) — Ring3 → Ring0
     ▼
Kernel Ring0 (fs/exofs/, process/, signal/, memory/)
 
Couche parallèle Ring3 :
exo-rt (fork de redox-rt) — compose les primitives kernel
     │ fork() : SYS_PROC_FORK + setup_child_stack + atfork handlers
     │ exec() : prépare argv/envp/auxv + SYS_PROC_EXEC
     │ signal : écriture SignalTcb AtomicU64 (0 syscall)
     └─→ utilisé PAR exo-libc, pas par les applis directement

6.2 — Dépôts et phases
Dépôt	Source	Travail	Phase	Licence
exo-os/musl-exo	musl 1.2.5	Adapter arch/x86_64/syscall_arch.h — remplacer ~50 __NR_xxx	Phase 1	MIT
exo-os/exo-rt	redox-os/redox-rt	Remplacer appels 'libredox' par nos syscalls	Phase 2	MIT
exo-os/exo-libc	redox-os/relibc	Écrire src/platform/exo_os/mod.rs (~2000 SLoC) impl Pal for ExoOs	Phase 2	MIT
exo-os/exo-libm	openlibm	Aucun — drop-in quasi-complet, fonctions mathématiques POSIX	Phase 1	MIT+BSD
exo-os/exo-alloc	dlmalloc-rs	Configurer backend mmap() → SYS_MEM_MAP	Phase 2	CC0
exo-os/exo-toolchain	rustc + libstd	Ajouter target x86_64-unknown-exo + fork std::sys::exo_os	Phase 3	MIT+Apache

6.3 — Règles libc
✅ LIB-01 — musl Phase1 : modifier syscall_arch.h UNIQUEMENT — ne pas toucher le code C
🔴 LIB-02 — INTERDIT : modifier les signal handlers musl — ils sont corrects et matures
✅ LIB-03 — exo-libc Phase2 : écrire src/platform/exo_os/ — couche Pal complète
✅ LIB-04 — Pal::open() = 2 syscalls : PATH_RESOLVE(path) → ObjectId → OBJECT_OPEN(oid, flags)
🔴 LIB-05 — INTERDIT : exposer ObjectId dans l'API POSIX — open() retourne un fd POSIX standard
✅ LIB-07 — exo-rt fork() : exo-rt prépare la stack child, kernel fait le CoW — division correcte
🔴 LIB-09 — INTERDIT : cloner relibc sans adapter la couche Pal — les schemes Redox ne fonctionnent pas
🔴 LIB-11 — INTERDIT : patcher relibc/src/header/ — impl Pal for ExoOs dans exo_os/ uniquement
🔴 LIB-13 — INTERDIT : exo-rt appelle mmap() pour CoW dans fork() — c'est le kernel

7 — Bugs et Incohérences identifiés par analyse à froid
Ces problèmes ont été identifiés en croisant ExoFS v3 avec l'architecture v6, les docs kernel DOC1-10 et les audits Gemini/Z-AI. Ils ne crashent pas immédiatement mais créent des comportements incorrects ou des failles.

🐛 BUG BUG-01 — CRITIQUE — musl open() incompatible — LIB-01 dit 'modifier syscall_arch.h uniquement'. Mais __NR_open = 500 ne fonctionne pas : musl appelle syscall(500, path, flags) en UN seul appel. ExoFS nécessite DEUX appels (PATH_RESOLVE puis OBJECT_OPEN). Solution : créer SYS_EXOFS_OPEN_BY_PATH = 519 (syscall combiné Ring0 qui enchaîne les deux).

// ❌ INCOHÉRENT — musl Phase1 avec __NR_open = 500
// musl génère : syscall(500, path, flags)  ← UN seul appel
// Mais PATH_RESOLVE retourne ObjectId, et OBJECT_OPEN a besoin de cet ObjectId
// → impossible à mapper sur un seul syscall avec __NR_open=500
 
// ✅ SOLUTION — ajouter SYS_EXOFS_OPEN_BY_PATH = 519
// SYS_EXOFS_OPEN_BY_PATH(path, path_len, flags, mode) → fd
// Implémenté dans fs/exofs/syscall/open_by_path.rs
// Enchaîne path_resolve() + object_open() en Ring0 atomiquement
// musl : #define __NR_open 519  // pointe vers le syscall combiné


🐛 BUG BUG-02 — CRITIQUE — getdents64 sans numéro — SYS_EXOFS_READDIR est absent de la liste 500-518. Le document écrit littéralement '#define __NR_getdents64 ???'. Sans ce syscall, ls, find, opendir() ne fonctionnent pas. Le numéro 519 est recommandé si open_by_path prend 519 — sinon choisir 520.


🐛 BUG BUG-03 — SYS_PROC_CLONE et SYS_PROC_EXEC non définis — exo-rt référence SYS_PROC_CLONE et SYS_PROC_EXEC mais syscall/numbers.rs liste fork=57 et execve=59 (POSIX inchangés). Ce sont probablement des aliases mais ils ne sont jamais déclarés dans numbers.rs. exo-rt ne peut pas compiler sans ces constantes.


🐛 BUG BUG-04 — CRITIQUE — do_exec() sans %fs — PROC-10 — do_exec() liste 8 étapes mais n'initialise jamais %fs (base TLS). Tout accès TLS dans exo-libc (errno, __errno_location, pthread clés) cause un segfault immédiat. ARCH_SET_FS ou wrmsrl(IA32_FS_BASE, tls_addr) manquant entre l'étape 7 (stack) et le jump_to_entry.


🐛 BUG BUG-05 — SYSRETQ sans vérification RCX canonique — Absent du v3. Errata Intel/AMD documenté : si userspace place une adresse non-canonique dans RCX avant SYSCALL, SYSRETQ fault en Ring0. Exploitable pour escalade de privilèges. Vérifier is_canonical(rcx) avant SYSRETQ. Si non-canonique → SIGSEGV au processus.


🐛 BUG BUG-06 — SEC-08 référence preuve Coq supprimée — SEC-08 : 'Délégation capability : droits_délégués ⊆ droits_délégateur — PROP-3 prouvée Coq'. La preuve Coq est supprimée en architecture v6. La règle reste vraie mais la justification est obsolète. Remplacer par : vérification proptest + INVARIANTS.md.


🐛 BUG BUG-07 — SYS-12 sévérité trop faible — SYS-12 marque comme ⚠️ ATTENTION le cas 'syscall depuis IRQ handler'. Cela devrait être ❌ INTERDIT. Un syscall depuis un IRQ est impossible architecturalement (Ring0 déjà, stack incorrecte, contexte non-préemptif) → undefined behavior immédiat, pas juste une attention.


🐛 BUG BUG-08 — EMERGENCY_POOL_SIZE contradictoire — ExoFS v3 (section modifications requises) dit augmenter EMERGENCY_POOL_SIZE à 96. Notre analyse kernel (Z-AI CVE-EXO-004) exige 256 minimum avec limite par processus. Ces deux valeurs sont en contradiction directe. La valeur correcte est 256 (+ limite 32 par processus).


🐛 BUG BUG-09 — PROC-03 signal pendant exec — non implémenté — PROC-03 identifie le bug : signal livré entre load ELF et reset TCB → handler pointe vers code de l'ANCIEN processus. La correction (bloquer tous signaux sauf SIGKILL pendant exec) est mentionnée dans la table d'erreurs silencieuses mais ABSENTE des étapes de do_exec().

7.1 — Résumé des bugs par priorité
ID	Priorité	Impact	Correction
BUG-01	P0	open() ne fonctionne pas en Phase1	Ajouter SYS_EXOFS_OPEN_BY_PATH = 519
BUG-02	P0	ls/find/opendir() impossibles	Ajouter SYS_EXOFS_READDIR (519 ou 520)
BUG-04	P0	Segfault immédiat à l'entrée exo-libc	arch::set_fs_base() dans do_exec() étape 9
BUG-05	P0	Exploitable Ring0 fault via SYSRETQ	verify_rcx_canonical_or_sigsegv() avant sysretq
BUG-03	P1	exo-rt ne compile pas	Déclarer SYS_PROC_CLONE + SYS_PROC_EXEC dans numbers.rs
BUG-09	P1	Signal handler exploit inter-exec	block_all_except_kill() dans do_exec() étape 3
BUG-06	P2	Référence architecture obsolète	Remplacer 'PROP-3 prouvée Coq' par proptest+INVARIANTS.md
BUG-07	P2	Documentation incorrecte	Changer ⚠️ ATTENTION en ❌ INTERDIT pour SYS-12
BUG-08	P1	Pool trop petit → DoS	Aligner sur 256 + limite par processus (doc kernel)

8 — Checklist syscall avant commit
#	Vérification	Modules
C-01	SYSCALL_TABLE[0..499] = POSIX, [500..518] = ExoFS, [519..] = ENOSYS	syscall/table.rs
C-02	bounds check rax >= TABLE_SIZE → -ENOSYS avant dispatch	syscall/table.rs
C-03	SWAPGS à l'entrée ET à la sortie du trampoline	syscall/trampoline.asm
C-04	verify_rcx_canonical() avant SYSRETQ — errata Intel/AMD	syscall/trampoline.asm
C-05	copy_from_user() sur TOUT pointeur Ring3 dans handlers/	handlers/*
C-06	handlers/ = thin wrappers — zéro logique métier	handlers/*
C-07	verify_cap() appelé AVANT délégation dans fs_posix.rs	handlers/fs_posix.rs
C-08	errno.rs couvre TOUS les variants — 15 entrées minimum	syscall/errno.rs
C-09	rax < 0 pour erreurs — jamais errno positif	syscall/errno.rs
C-10	SYS_EXOFS_OPEN_BY_PATH (519) implémenté	fs/exofs/syscall/open_by_path.rs
C-11	SYS_EXOFS_READDIR (520) implémenté	fs/exofs/syscall/readdir.rs
C-12	SYS_PROC_CLONE et SYS_PROC_EXEC dans numbers.rs	syscall/numbers.rs
C-13	do_exec() étape 9 : arch::set_fs_base() avant jump_to_entry	process/exec.rs
C-14	do_exec() étape 3 : block_all_except_kill() avant chargement ELF	process/exec.rs
C-15	SignalTcb.handlers = [SigactionEntry; 64] valeur (pas AtomicPtr)	signal/tcb.rs
C-16	sigreturn : vérification magic 0x5349474E avant restore	signal/trampoline.asm
C-17	fork() : CoW dans kernel, TLB shootdown IPI tous CPUs	process/fork.rs
C-18	FD_CLOEXEC révoqué auto à exec() — cap_table.revoke_cloexec()	process/exec.rs
C-19	auxv contient AT_SIGNAL_TCB + AT_RANDOM(16B) + AT_SYSINFO_EHDR	process/auxv.rs
C-20	ELF : W^X strict — PF_W|PF_X simultanés = EACCES	process/elf/segments.rs
C-21	exo-libc Pal::open() = 2 appels — ObjectId jamais exposé à l'app	exo-libc
C-22	musl Phase1 : __NR_open pointe vers SYS_EXOFS_OPEN_BY_PATH	musl-exo
C-23	EMERGENCY_POOL_SIZE = 256, limite 32 par processus	memory/physical/frame/pool.rs
C-24	SEC-08 texte mis à jour : proptest + INVARIANTS.md (plus Coq)	security/capability/

