EXO-OS KERNEL
Règles, Erreurs & Corrections
DOC1 → DOC10 · Architecture v6 · Référence Indépendante
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Ce document est INDÉPENDANT d'ExoFS Reference v3

A — Périmètre et Version de Référence
Ce document couvre exclusivement le kernel Ring 0 d'Exo-OS tel que défini dans les documents DOC1 à DOC10 et la doc spéciale architecture v6.

ℹ️ PÉRIMÈTRE — ExoFS Reference v3 est un document SÉPARÉ avec sa propre gestion d'erreurs. Ne pas dupliquer ses règles ici.

Version de référence : Architecture v6
La doc_speciale v6 est la version FINALE pour les points suivants :
⚠️ v6-01 — ipc/capability_bridge/ est SUPPRIMÉ. ipc/ appelle security::access_control::check_access() directement.
⚠️ v6-02 — La preuve formelle Coq/TLA+ est supprimée. Remplacée par INVARIANTS.md + proptest + CI grep.
🔴 v6-03 — DOC5 (ipc/ via capability_bridge) est OBSOLÈTE sur ce point. Toute règle mentionnant capability_bridge est périmée.

Hiérarchie des couches (non négociable)
Couche	Module	Dépend de	Peut être appelé par
0	memory/	RIEN (uniquement arch/)	scheduler, ipc, fs, process
1	scheduler/	memory/ uniquement	ipc, fs, process, arch
1.5	process/	memory + scheduler	ipc, fs, arch/syscall
2a	ipc/	memory + scheduler + security	fs (via shim)
TCB	security/	memory + scheduler	ipc, fs, process
3	fs/	memory + scheduler + security	userspace via syscall

RÈGLE ABSOLUE — toute dépendance remontante est un bug architectural.

B — Règles Transversales (tous modules)
B1 — Lock Ordering (regle_bonus.md)
🔴 LOCK-01 — Ordre STRICT : IPC(1) → Scheduler(2) → Memory(3) → FS(4)
🔴 LOCK-02 — INTERDIT : tenir un lock N+1 et acquérir N. Sans exception.
🔴 LOCK-03 — Ordre SMP : acquérir les locks de deux CPUs dans l'ordre CPU_ID croissant (anti ABBA deadlock).
⚠️ LOCK-04 — Ordre inodes : jamais acquérir inode A → inode B sans ordre déterministe (ID croissant).

NOUVEAU — Règle 5 : Pré-allocation de frames avant lock FS
🔴 LOCK-05 — Le writeback thread DOIT réserver les frames physiques nécessaires AVANT d'acquérir EPOCH_COMMIT_LOCK ou tout lock FS (niveau 4). Sinon : violation Lock-Ordering (FS tient N4 et alloue N3).
// ❌ FAUTIF — violation lock ordering
let _lock = epoch_commit_lock.lock();  // Niveau 4
let frame = buddy::alloc_pages(0, flags)?;  // Niveau 3 sous lock FS !
 
// ✅ CORRECT
let reservation = memory::physical::frame::reserve_for_commit(n_needed)?;
// ↑ Réserve les frames AVANT de prendre le lock FS
let _lock = epoch_commit_lock.lock();  // Niveau 4
let frame = reservation.take();  // Utilise la réservation — zéro alloc sous lock

B2 — Zones No-Alloc (regle_bonus.md)
🔴 NOALLOC-01 — INTERDIT d'utiliser alloc, Vec, Box, Rc, Arc dans : scheduler/core/, handlers ISR, code sous preemption_disable().
✅ NOALLOC-02 — Utiliser uniquement : stack, static buffers, ring::slot, per-CPU pools.
✅ NOALLOC-03 — EmergencyPool (WaitNodes) = seule exception d'allocation indirecte dans ISR/preempt.

B3 — Contrat unsafe (regle_bonus.md)
🔴 UNSAFE-01 — Tout bloc unsafe { } DOIT être précédé de // SAFETY: <raison>. Rejet automatique si absent.
// ❌ FAUTIF
unsafe { ptr.write(value); }
 
// ✅ CORRECT
// SAFETY: ptr est valide car alloué depuis EmergencyPool initialisé
//         et slot est exclusif (CAS atomic réussi)
unsafe { ptr.write(value); }

B4 — RAII obligatoire
🔴 RAII-01 — PreemptGuard pour preempt_disable/enable — JAMAIS les appels directs.
🔴 RAII-02 — SpinLockGuard pour lock/unlock — JAMAIS manuels.
🔴 RAII-03 — Si une exception / panique se produit entre disable et enable manuel → compteur corrompu → deadlock définitif.

B5 — RunQueue intrusive (NOUVEAU — Gemini G3)
🔴 SCHED-INTRU — scheduler/core/runqueue.rs DOIT utiliser une liste intrusive. Les nœuds next/prev doivent vivre DIRECTEMENT dans le TCB.
Raison critique : un ISR DMA réveille un thread → si RunQueue = Vec → push() = allocation → OOM panic dans l'interruption. Avec liste intrusive = 2 réassignations de pointeurs, zéro allocation.
// ❌ FAUTIF — allocation lors du wakeup depuis ISR
pub struct RunQueue {
    tasks: Vec<TaskRef>,  // push() = alloc → INTERDIT en ISR
}
 
// ✅ CORRECT — intrusive, nœuds dans le TCB
pub struct ThreadControlBlock {
    // ...
    pub rq_next: AtomicPtr<ThreadControlBlock>,  // ← dans le TCB
    pub rq_prev: AtomicPtr<ThreadControlBlock>,  // ← dans le TCB
}
pub struct RunQueue {
    head: AtomicPtr<ThreadControlBlock>,  // liste intrusive — zéro alloc
}

B6 — assert! anti-preemption avant block_current()
🔴 PREEMPT-BLOCK — INTERDIT d'appeler block_current() avec un PreemptGuard actif (preempt_count > 0). Résultat : deadlock garanti car le scheduler ne peut pas préempter.
// Dans wait_queue::wait() et futex_wait()
// AJOUTER CETTE ASSERTION AVANT de bloquer :
debug_assert!(
    percpu::preempt_count().load(Ordering::Relaxed) == 0,
    "block_current() appelé avec PreemptGuard actif — deadlock garanti"
);
scheduler::block_current(timeout);

C — Module memory/ (Couche 0)
memory/ est la couche 0 absolue. Elle ne doit importer aucun autre module kernel.
C1 — Règles fondamentales
🔴 MEM-01 — Aucun import de scheduler/, ipc/, fs/, process/ dans memory/. Violation = bug architectural immédiat.
🔴 MEM-02 — EmergencyPool initialisé EN PREMIER (step 1 du boot, avant buddy).
🔴 MEM-03 — FutexTable = UNIQUE dans memory/utils/futex_table.rs. Indexée par adresse PHYSIQUE (pas virtuelle).
🔴 MEM-04 — TLB shootdown synchrone IPI AVANT free_pages(). Ne jamais libérer un frame avant ACK de tous les CPUs.
🔴 MEM-05 — DMA frames = FrameFlags::DMA_PINNED jusqu'à wait_dma_complete().
🔴 MEM-06 — Pages IPC/SHM = FrameFlags::NO_COW + SHM_PINNED (les deux obligatoires).
🔴 MEM-07 — INTERDIT split une huge page marquée DMA_PINNED.
⚠️ MEM-08 — OOM killer = thread dédié. JAMAIS appelé depuis le hot path de reclaim.
🔴 MEM-09 — DmaWakeupHandler = trait abstrait. memory/ n'importe JAMAIS process/ directement.
🔴 MEM-10 — AllocFlags::ATOMIC pour toute allocation en contexte IRQ (uniquement per-CPU pool, jamais buddy).
⚠️ MEM-11 — Allocations en contexte de reclaim : PF_MEMALLOC sur le thread reclaimeur pour éviter deadlock récursif.
🔴 MEM-12 — CoW fork : flush TLB parent AVANT retour fork().

C2 — NOUVEAU : FUTEX hash — DoS par collision (Z-AI INTEG-002)
🔴 MEM-FUTEX — FUTEX_HASH_BUCKETS = 4096 minimum. Hash = SipHash-keyed (graine aléatoire au boot). Avec 256 buckets et hash trivial, un attaquant concentre ses futex sur 1 bucket → O(n) lookup → DoS.
// ❌ FAUTIF — 256 buckets + hash trivial = DoS
pub const FUTEX_HASH_BUCKETS: usize = 256;
fn hash(addr: u64) -> usize {
    ((addr >> 2) ^ (addr >> 12)) as usize & (FUTEX_HASH_BUCKETS - 1)
}
 
// ✅ CORRECT — 4096 buckets + SipHash keyed
pub const FUTEX_HASH_BUCKETS: usize = 4096;  // pouvoir de 2
static HASH_SEED: spin::Once<[u8; 16]> = spin::Once::new();
 
fn hash(phys_addr: u64) -> usize {
    let seed = HASH_SEED.get().expect("seed non initialisée");
    let mut hasher = SipHasher13::new_with_key(seed);
    hasher.write_u64(phys_addr);
    hasher.finish() as usize & (FUTEX_HASH_BUCKETS - 1)
}
// HASH_SEED initialisée depuis security::crypto::rng au boot (step 18)

C3 — NOUVEAU : DMA IRQ → wakeup sans lock (Z-AI ARCH-001 + Gemini G3)
🔴 MEM-DMA-IRQ — L'ISR DMA completion DOIT libérer tout lock memory/dma/ AVANT d'appeler DmaWakeupHandler::wakeup_thread(). Sinon : lock mémoire (niveau 3) tenu lors d'un wake scheduler (niveau 2) = violation lock ordering.
// ❌ FAUTIF — lock DMA tenu pendant wakeup scheduler
fn dma_completion_irq(req: &DmaRequest) {
    let _lock = DMA_RING_LOCK.lock();  // lock DMA tenu
    dma_wakeup::wakeup_thread(req.thread_id, Ok(()));  // appel scheduler sous lock !
}
 
// ✅ CORRECT — wakeup différé, hors lock
fn dma_completion_irq(req: &DmaRequest) {
    {
        let _lock = DMA_RING_LOCK.lock();
        req.mark_complete();  // marquer complété sous lock
    }  // ← lock libéré ICI
    // wakeup APRÈS libération de tout lock DMA
    dma_wakeup::wakeup_thread(req.thread_id, Ok(()));
}

D — Module scheduler/ (Couche 1)
D1 — Règles fondamentales
🔴 SCHED-01 — Dépend UNIQUEMENT de memory/. Jamais ipc/, fs/, process::signal.
🔴 SCHED-02 — signal/ est ABSENT de scheduler/ — déplacé dans process/ (DOC1).
🔴 SCHED-03 — futex.rs est ABSENT de scheduler/ — unique dans memory/utils/futex_table.rs.
🔴 SCHED-04 — PreemptGuard RAII obligatoire — jamais preempt_disable()/enable() directs.
🔴 SCHED-05 — WaitNode depuis EmergencyPool uniquement (jamais heap).
🔴 SCHED-06 — switch_asm.s : sauvegarder r15 OBLIGATOIREMENT (utilisé par ext4plus/inode/ops.rs).
🔴 SCHED-07 — MXCSR + x87 FCW sauvegardés explicitement dans switch_asm.s, indépendamment de XSAVE.
🔴 SCHED-08 — CR3 switché DANS switch_asm AVANT restauration des registres du nouveau thread (KPTI).
✅ SCHED-09 — Lazy FPU : save AVANT switch_asm, mark 'non chargé' APRÈS (pour le nouveau thread).
⚠️ SCHED-10 — Migration SMP : threads RT non migrés sans vérification cpumask stricte.
🔴 SCHED-11 — pick_next_task() : zéro allocation, zéro sleep.
🔴 SCHED-12 — RunQueue = liste intrusive (nœuds dans TCB). Voir règle SCHED-INTRU (Section B5).
🔴 SCHED-13 — signal_pending = AtomicBool dans TCB. scheduler LIT seulement, process/signal/ ÉCRIT.

D2 — Context switch ASM — erreurs fréquentes
context_switch_asm:
    # Sauvegarder callee-saved (ABI System V)
    push %rbx
    push %rbp
    push %r12
    push %r13
    push %r14
    push %r15          # ← OBLIGATOIRE (ext4plus/inode/ops.rs utilise r15)
 
    sub $4, %rsp
    stmxcsr (%rsp)     # ← MXCSR explicite (indépendant de XSAVE)
    sub $4, %rsp
    fstcw (%rsp)       # ← x87 FCW explicite
 
    mov %rsp, (%rdi)   # Sauvegarder rsp thread sortant
 
    # CR3 switché ICI — AVANT de charger nouveau rsp
    cmp %rdx, %cr3
    je .skip_cr3
    mov %rdx, %cr3     # ← AVANT restauration regs (KPTI)
.skip_cr3:
    mov %rsi, %rsp     # Charger rsp nouveau thread
    # Restaurer dans l'ordre inverse...

D3 — EmergencyPool — DoS par épuisement (Z-AI CVE-EXO-004)
🔴 SCHED-POOL — 64 WaitNodes = insuffisant. Un attaquant avec 64 threads bloqués épuise le pool. Le expect() produit une kernel panic = DoS trivial.
// ❌ FAUTIF — expect() = kernel panic si pool épuisé
let node = emergency_pool::alloc_wait_node()
    .expect("EmergencyPool épuisé");  // kernel panic → DoS trivial
 
// ✅ CORRECT — retour Err() propre + limite par processus
// 1. Augmenter EMERGENCY_POOL_SIZE à 256 minimum
pub const EMERGENCY_POOL_SIZE: usize = 256;
 
// 2. Limite par processus
static WAITS_PER_PROCESS: PerCpu<AtomicU32> = PerCpu::new();
const MAX_WAITS_PER_PROCESS: u32 = 32;
 
// 3. Retour Err() propre, jamais panic
let node = emergency_pool::alloc_wait_node()
    .ok_or(WaitError::EmergencyPoolExhausted)?;

E — Module process/ + signal/ (Couche 1.5)
E1 — Règles fondamentales
🔴 PROC-01 — exec() via trait ElfLoader abstrait — JAMAIS import direct fs/.
🔴 PROC-02 — DmaWakeupHandler impl dans process/state/wakeup.rs — enregistré au boot.
🔴 PROC-03 — signal/ géré dans process/ entièrement (déplacé depuis scheduler/, voir DOC1).
🔴 PROC-04 — signal_pending = AtomicBool dans TCB. ÉCRIT par process/signal/, LU par scheduler.
⚠️ PROC-05 — TCB ≤ 128 bytes (2 cache lines). const assert vérifié à la compilation.
🔴 PROC-06 — Livraison signal : au retour userspace UNIQUEMENT (arch/syscall.rs + arch/exceptions.rs).
⚠️ PROC-07 — zombie reaper = kthread dédié — jamais inline dans exit().
🔴 PROC-08 — fork() : flush TLB parent AVANT retour.
✅ PROC-09 — dma_completion_result = AtomicU8 dans TCB (requis par process/state/wakeup.rs).

E2 — Erreur corrigée : switch.rs appelle process/signal (DOC1)
// ❌ FAUTIF (original) — switch.rs (couche 1) appelle process::signal (couche 1.5)
// Commentaire erroné dans delivery.rs :
// "Appelé par : scheduler/core/switch.rs (retour préemption)"
 
// ✅ CORRECT
// switch.rs LIT uniquement signal_pending (AtomicBool) :
pub fn check_signal_pending(tcb: &ThreadControlBlock) -> bool {
    tcb.signal_pending.load(Ordering::Relaxed)
    // Si true → arch/exceptions.rs OU arch/syscall.rs orchestrent la livraison
    // switch.rs ne fait QUE lire — jamais appeler process::signal::*
}
 
// C'est arch/exceptions.rs qui livrait les signaux au retour préemption :
pub fn exception_return_to_user(tcb: &mut ThreadControlBlock) {
    if tcb.signal_pending.load(Ordering::Acquire) {
        process::signal::delivery::handle_pending_signals(tcb);  // arch/ peut appeler process/
    }
    // IRETQ
}

E3 — NOUVEAU : VMA TCB Signal — MAP_FIXED collision (Z-AI CVE-EXO-002)
🔴 PROC-VMA — La VMA contenant le SignalTcb DOIT être marquée VM_DONTCOPY | VM_DONTEXPAND dès exec(). Sinon : un attaquant peut appeler mmap(SIGNAL_TCB_VADDR, ..., MAP_FIXED) et écrire des signal frames dans sa propre mémoire.
// Dans process/lifecycle/exec.rs, après mapping du SignalTcb :
let vma = addr_space.find_vma_mut(signal_tcb_vaddr)?;
vma.flags.insert(VmaFlags::DONTCOPY);      // pas copié au fork()
vma.flags.insert(VmaFlags::DONTEXPAND);    // MAP_FIXED refusé sur cette plage
// Retourner ERESERVEDRANGE si un mmap() tente de chevaucher cette plage

E4 — TCB : AddressSpaceRef taille critique
⚠️ PROC-TCB-SIZE — AddressSpaceRef DOIT être 8 bytes (pointer mince, pas fat pointer). Si Arc<AddressSpace> = 16 bytes → cache line 2 déborde → assert size échoue. Utiliser *const AddressSpace ou un index u32.

F — Module ipc/ (Couche 2a)
F1 — Règles fondamentales
🔴 IPC-01 — SPSC ring : head et tail sur cache lines SÉPARÉES (CachePadded<AtomicU64>). Sans ça : false sharing → dégradation 10–100×.
🔴 IPC-02 — ipc/sync/futex.rs = délégation PURE à memory/utils/futex_table. Zéro logique futex locale.
🔴 IPC-03 — Pages SHM = FrameFlags::NO_COW + SHM_PINNED (les deux obligatoires).
🔴 IPC-04 — ipc/ appelle security::access_control::check_access() DIRECTEMENT (v6 — capability_bridge supprimé).
🔴 IPC-05 — ipc/ N'APPELLE PAS fs/ directement. Passer par fs/ipc_fs/ (pipefs, socketfs).
✅ IPC-06 — Fusion Ring : anti-thundering herd (batch adaptatif 1–64 messages par wakeup).
✅ IPC-07 — Fast IPC = fichier .s ASM, pas .rs.
🔴 IPC-08 — Spectre v1 : array_index_nospec() sur TOUS les accès indexés dans les ring buffers.

F2 — NOUVEAU : capability_bridge supprimé (doc_speciale v6)
// ❌ OBSOLÈTE (avant v6) — via capability_bridge
use crate::ipc::capability_bridge::bridge::verify_ipc_access;
verify_ipc_access(&table, token, endpoint_id, required)?;
 
// ✅ CORRECT (v6) — appel direct à security::access_control
use crate::security::access_control::checker::check_access;
use crate::security::access_control::object_types::ObjectKind;
check_access(&table, token, ObjectKind::Channel, required, "ipc::channel")?;
// ipc/capability_bridge/ n'existe plus — toute référence est un bug

F3 — Spectre v1 — exemple concret
// ❌ FAUTIF — accès indexé non protégé
pub fn pop(&self) -> Option<T> {
    let tail = self.tail.value.load(Ordering::Relaxed);
    let item = unsafe { self.buffer[tail as usize].read() };  // spéculatif !
    Some(item)
}
 
// ✅ CORRECT — array_index_nospec
pub fn pop(&self) -> Option<T> {
    let tail = self.tail.value.load(Ordering::Relaxed);
    let safe_idx = array_index_nospec(tail as usize, N);  // masque spéculatif
    let item = unsafe { self.buffer[safe_idx].read() };
    Some(item)
}

G — Module security/ (TCB — v6 sans preuve formelle)
G1 — Règles fondamentales
🔴 SEC-01 — security/capability/ = source de vérité UNIQUE pour toutes les vérifications de droits.
🔴 SEC-02 — verify() = point d'entrée unique. JAMAIS dupliquer dans un autre module.
🔴 SEC-03 — Révocation = génération++. O(1). JAMAIS parcours de tokens.
🔴 SEC-04 — Délégation = subset strict des droits. t2.rights = t1.rights & requested. Impossible d'obtenir plus.
🔴 SEC-05 — XChaCha20 sur TOUS les canaux inter-domaines.
⚠️ SEC-06 — KASLR actif — base kernel randomisée au boot.
🔴 SEC-07 — Retpoline sur TOUS les appels indirects hot path.
⚠️ SEC-08 — SSBD per-thread, switché avec le contexte.
ℹ️ SEC-09 — INVARIANTS.md + proptest + CI grep remplacent la preuve Coq (v6).
🔴 SEC-10 — CI grep : aucun module ne peut contourner security::capability::verify() — vérifié automatiquement.

G2 — NOUVEAU : gap boot APs (Z-AI CVE-EXO-001)
🔴 SEC-BOOT-GAP — Entre step 17 (security::capability::init) et step 18 (security::access_control::checker::init), les APs SMP ne doivent PAS tenter d'IPC. Ajouter un flag atomique SECURITY_READY.
// Dans security/mod.rs :
pub static SECURITY_READY: AtomicBool = AtomicBool::new(false);
 
// Séquence boot (arch/x86_64/smp/init.rs pour les APs) :
// AP spin-wait AVANT de tenter toute IPC
while !security::SECURITY_READY.load(Ordering::Acquire) {
    core::hint::spin_loop();
}
// Désormais sûr de faire de l'IPC
 
// Dans la séquence boot BSP (step 18) :
security::access_control::checker::init();
security::SECURITY_READY.store(true, Ordering::Release);  // ← APRÈS init

G3 — NOUVEAU : verify() constant-time
⚠️ SEC-CONST-TIME — verify() ne doit pas retourner plus vite pour un token totalement inexistant vs révoqué. Une différence de timing permet de distinguer les deux cas = info-leak.
// ❌ FAUTIF — early return révèle l'existence de l'objet
pub fn verify(table: &CapTable, token: CapToken) -> Result<(), CapError> {
    let entry = table.get(token.object_id)
        .ok_or(CapError::ObjectNotFound)?;  // ← retour rapide si objet inexistant
    if entry.generation != token.generation { return Err(CapError::Revoked); }
    Ok(())
}
 
// ✅ CORRECT — parcours complet même si absent
pub fn verify(table: &CapTable, token: CapToken) -> Result<(), CapError> {
    let entry_opt = table.get(token.object_id);
    // Comparaison constante — même si entry_opt = None, on compare quand même
    let stored_gen = entry_opt.map(|e| e.generation).unwrap_or(u32::MAX);
    if stored_gen != token.generation { return Err(CapError::Denied); }
    entry_opt.ok_or(CapError::Denied)?;
    Ok(())
}

H — Module fs/ (Couche 3)
H1 — Règles fondamentales
🔴 FS-01 — Relâcher lock inode AVANT sleep (release-before-sleep). JAMAIS tenir lock inode pendant une attente I/O.
⚠️ FS-02 — io_uring : EINTR propre avec IORING_OP_ASYNC_CANCEL.
🔴 FS-03 — IPC via fs/ipc_fs/ UNIQUEMENT (pipefs, socketfs). ipc/ n'importe jamais fs/ directement.
🔴 FS-04 — Capabilities via security::access_control::check_access() (pas capability_bridge).
🔴 FS-05 — ElfLoader trait enregistré par fs/ pour process/exec — pas d'import direct.
✅ FS-06 — Slab shrinker enregistré auprès de memory/utils/shrinker.rs.
✅ FS-07 — Blake3 checksums sur toutes les écritures ext4plus.
🔴 FS-08 — WAL (Data=Ordered) : données physiques AVANT commit journal.
🔴 FS-09 — Delayed alloc : blocs physiques alloués au writeback SEULEMENT, jamais au write().
🔴 FS-10 — Incompat flags ext4plus : EXO_BLAKE3 | EXO_DELAYED | EXO_REFLINK obligatoires.
🔴 FS-11 — ext4plus ≠ ext4 classique. Ne jamais mélanger les deux formats.
🔴 FS-12 — ext4 classique (fs/drivers/ext4/) : jamais monter en RW si journal needs_recovery = true.

H2 — Data=Ordered : séquence obligatoire
// SÉQUENCE OBLIGATOIRE pour toute écriture ext4plus :
 
// ÉTAPE 1 : Données → emplacement final (DMA direct)
dma::submit_write(block_range, &dirty_pages)?;
dma::wait_completion()?;  // ← barrière physique (ACK disque)
 
// ÉTAPE 2 : Métadonnées → journal WAL
let txn = journal::begin_transaction();
txn.record_inode_update(inode_id, block_range);
txn.set_data_barrier_passed();  // données confirmées
txn.commit()?;  // ← INTERDIT si data_barrier_passed = false
 
// ❌ FAUTIF — journal avant données physiques
// txn.commit()?;  // commit journal
// dma::submit_write(block_range, ..)?;  // données APRÈS = corruption potentielle

H3 — NOUVEAU : writeback thread doit pré-allouer les frames
🔴 FS-PREALLOC — Le writeback thread DOIT appeler reserve_for_commit(n) AVANT d'acquérir tout lock FS. Voir LOCK-05 (Section B1).

I — Module arch/ + Boot
I1 — NOUVEAU : SYSRET avec RCX non-canonique (errata Intel/AMD)
🔴 ARCH-SYSRET — Si RCX contient une adresse non-canonique au moment du SYSRET, le CPU fault en Ring 0. Vérifier RCX avant SYSRET. Si non-canonique → SIGSEGV au processus, pas kernel fault.
// Dans arch/x86_64/syscall.rs — avant SYSRET :
 
/// Vérifier qu'une adresse est canonique x86_64
/// Adresse canonique : bits 63..47 sont tous identiques (signe-étendu bit 47)
#[inline(always)]
fn is_canonical(addr: u64) -> bool {
    let sign_bits = addr >> 47;
    sign_bits == 0 || sign_bits == 0x1FFFF
}
 
pub fn syscall_return_to_user(tcb: &mut ThreadControlBlock, user_rip: u64) {
    if !is_canonical(user_rip) {
        // Adresse de retour non-canonique → SIGSEGV, jamais SYSRET
        process::signal::delivery::send_signal(tcb, Signal::SIGSEGV);
        return;  // ← retour vers userspace via iret classique, pas SYSRET
    }
    // SYSRET sûr ici
}

I2 — NOUVEAU : Séquence boot — flag SECURITY_READY pour APs
🔴 BOOT-SEC — Les APs doivent attendre SECURITY_READY avant toute IPC. Voir aussi SEC-BOOT-GAP (Section G2).
I3 — Séquence boot : erreurs de step
Erreur originale corrigée : security::capability::init() doit précéder process::core::registry::init().
Step	Action	Statut
1	memory::physical::frame::emergency_pool::init()	EN PREMIER ABSOLU
3–5	buddy, heap, futex_table::init()	avant tout autre module
17	security::capability::init()	AVANT process/ (step 19)
18	security::crypto::rng::init() + SECURITY_READY flag APs	AJOUT v6 — gap APs
19	process::core::registry::init()	APRÈS security/
20	process::state::wakeup::register_with_dma()	enregistrer DmaWakeupHandler
26	arch::x86_64::smp::start_aps()	APs démarrés APRÈS SECURITY_READY

J — DOC10 : Bootloader · Loader · Drivers · Userspace
J1 — Règles bootloader (exo-boot/)
🔴 BOOT-01 — exo-boot = binaire séparé du kernel. Aucune dépendance de code partagé.
🔴 BOOT-02 — Secure Boot : vérification signature Ed25519 AVANT chargement kernel. Refus si invalide.
🔴 BOOT-03 — BootInfo = contrat formel. Version checkée par kernel (magic + version).
🔴 BOOT-04 — ExitBootServices = point de non-retour. Aucun UEFI Boot Service après ce point.
⚠️ BOOT-05 — PIE + KASLR : adresse kernel randomisée par le bootloader.
⚠️ BOOT-06 — Entropy 64 bytes fournie au kernel (CSPRNG + KASLR + hash seeds).
🚫 INTERDIT : Charger un kernel non signé si Secure Boot actif.
🚫 INTERDIT : BootInfo avec champs non initialisés (zéro-fill obligatoire).

J2 — Règles loader (Ring 3)
🔴 LDR-01 — Loader = processus Ring 3. Aucun code kernel.
🔴 LDR-02 — Vérification signature binaire AVANT tout chargement.
🔴 LDR-03 — W^X strict : JAMAIS mapper une page W+X simultanément.
⚠️ LDR-04 — ASLR obligatoire pour tous les PIE.
✅ LDR-05 — TLS initialisé via PT_TLS avant entry point.

J3 — Règles Shield (servers/shield/)
🔴 SHL-01 — Shield est lui-même sandboxé (self_isolate() au boot).
🔴 SHL-02 — Watchdog obligatoire — protection ne tombe pas si Shield crash.
✅ SHL-03 — ML inference ≤ 100µs, modèle ≤ 2MB.
🔴 SHL-04 — Mise à jour modèle ML = offline uniquement. JAMAIS en Ring 0 runtime.
⚠️ SHL-05 — Hooks = analyse, pas blocage systématique sans score d'anomalie.
🚫 INTERDIT : Shield sans sandbox propre.
🚫 INTERDIT : Ré-implémenter XChaCha20 dans Shield (appeler kernel).

K — Incohérences identifiées entre les documents
Ces points sont des conflits entre versions de documents. La version v6 (doc_speciale) prime toujours sur les versions antérieures.

Conflit	DOC ancien	Version v6 (fait foi)	Impact
capability_bridge	DOC5 : ipc/ → ipc/capability_bridge/ → security/	Supprimé. ipc/ → security::access_control::check_access() direct	CRITIQUE : toute règle mentionnant capability_bridge est obsolète
emergency_pool.rs	DOC2 : fichier séparé emergency_pool.rs	Arborescence v6 : fusionné dans frame/pool.rs conceptuellement	Clarifier que pool d'urgence (WaitNodes) ≠ per-CPU pools
wakeup_iface.rs	DOC2 : memory/dma/core/wakeup_iface.rs	Arborescence v6 : dma/completion/wakeup.rs	Fichier renommé mais principe inchangé (trait abstrait)
Preuve Coq	DOC7 : périmètre TCB prouvé, PROOF_SCOPE.md	Supprimé. INVARIANTS.md + proptest + CI grep	Toute règle basée sur périmètre Coq est obsolète
fs/ipc_fs/shim.rs	DOC5 : shim.rs pour IPC→FS	Arborescence v6 : fs/ipc_fs/pipefs.rs + socketfs.rs	Principe inchangé : ipc/ n'importe jamais fs/ directement
Ordre boot steps 17/19	Original : process/ avant security/	security::capability::init() AVANT process::core::registry::init()	CRITIQUE : process/ dépend de security/

L — Checklist avant commit (40 points)
Ces vérifications doivent être effectuées avant tout commit sur le kernel Ring 0.

#	Vérification	Modules concernés
V-01	Aucun use crate::scheduler dans memory/	memory/*
V-02	EmergencyPool init = step 1 du boot	boot seq
V-03	FutexTable = une seule instance dans memory/utils/	memory, ipc, scheduler
V-04	TLB shootdown avant free_pages()	memory/virtual/
V-05	DMA frames DMA_PINNED jusqu'à ACK	memory/dma/
V-06	SHM pages = NO_COW + SHM_PINNED	ipc/shared_memory/
V-07	RunQueue = liste intrusive (nœuds dans TCB)	scheduler/core/runqueue.rs
V-08	PreemptGuard RAII — jamais direct	scheduler, ipc
V-09	switch_asm.s : r15 + MXCSR + x87 FCW + CR3 avant regs	scheduler/asm/
V-10	WaitNode depuis EmergencyPool (pas heap)	scheduler/sync/
V-11	EmergencyPool : Err() jamais expect()	scheduler/sync/
V-12	EMERGENCY_POOL_SIZE ≥ 256 + limite par processus	memory/physical/frame/
V-13	signal/ entièrement dans process/ (absent de scheduler/)	process/signal/
V-14	signal_pending AtomicBool dans TCB — scheduler lit, process écrit	TCB
V-15	TCB ≤ 128 bytes (const assert à la compilation)	process/core/tcb.rs
V-16	AddressSpaceRef = 8 bytes (pas fat pointer)	process/core/tcb.rs
V-17	VMA SignalTcb = VM_DONTCOPY | VM_DONTEXPAND	process/lifecycle/exec.rs
V-18	exec() via trait ElfLoader abstrait (pas import fs/)	process/lifecycle/exec.rs
V-19	ipc/ → security::access_control::check_access() DIRECT	ipc/*
V-20	SPSC head/tail sur cache lines séparées (CachePadded)	ipc/ring/spsc.rs
V-21	ipc/sync/futex.rs = délégation pure à memory/	ipc/sync/futex.rs
V-22	array_index_nospec() sur accès ring buffers IPC	ipc/ring/*
V-23	ipc/ n'importe pas capability_bridge/ (supprimé v6)	ipc/*
V-24	security/capability/verify() = point d'entrée unique	security/capability/
V-25	SECURITY_READY flag atomique après init security (step 18)	security/mod.rs
V-26	APs spin-wait sur SECURITY_READY avant IPC	arch/smp/init.rs
V-27	verify() constant-time (pas d'early return révélateur)	security/capability/verify.rs
V-28	Lock ordering : pré-alloc frames avant lock FS (LOCK-05)	fs/cache/writeback.rs
V-29	FS Data=Ordered : données physiques AVANT commit journal	fs/integrity/journal.rs
V-30	Delayed alloc : blocs alloués au writeback, jamais au write()	fs/cache/writeback.rs
V-31	Incompat flags ext4plus dans superblock	fs/ext4plus/superblock.rs
V-32	ext4 classique : refus si needs_recovery = true	fs/drivers/ext4/journal.rs
V-33	DMA IRQ : wakeup APRÈS libération lock DMA	memory/dma/completion/
V-34	FUTEX : 4096 buckets + SipHash-keyed	memory/utils/futex_table.rs
V-35	arch/syscall.rs : vérif RCX canonique avant SYSRET	arch/x86_64/syscall.rs
V-36	Ordre boot : security::capability (17) avant process (19)	boot seq
V-37	DMA wakeup via trait abstrait (pas import direct process/)	memory/dma/
V-38	block_current() jamais sous PreemptGuard actif	scheduler/sync/, ipc/sync/
V-39	exo-boot : signature Ed25519 avant chargement kernel	exo-boot/
V-40	Shield : self_isolate() + watchdog au boot	servers/shield/

M — Résumé Flash — 25 règles inviolables
Ces 25 règles couvrent les erreurs les plus fréquentes et les plus graves. À mémoriser avant toute session de génération de code.

#	Règle	Sévérité
1	memory/ ne dépend de RIEN d'autre que arch/	CRITIQUE
2	EmergencyPool initialisé EN PREMIER (avant buddy)	CRITIQUE
3	TLB shootdown synchrone AVANT free_pages()	CRITIQUE
4	RunQueue = liste intrusive dans TCB — zéro alloc en ISR	CRITIQUE
5	Zones no-alloc : scheduler/core/, ISR, preempt_disable()	CRITIQUE
6	Lock ordering IPC(1)→Sched(2)→Mem(3)→FS(4)	CRITIQUE
7	Pré-alloc frames AVANT tout lock FS (LOCK-05)	CRITIQUE
8	PreemptGuard RAII — jamais preempt_disable/enable directs	CRITIQUE
9	JAMAIS block_current() sous PreemptGuard actif	CRITIQUE
10	switch_asm.s : r15 + MXCSR + x87 FCW + CR3 avant regs	CRITIQUE
11	signal/ dans process/ uniquement — absent de scheduler/	CRITIQUE
12	signal_pending : scheduler LIT, process/signal ÉCRIT	CRITIQUE
13	FutexTable UNIQUE dans memory/utils/ — 4096 buckets SipHash	CRITIQUE
14	ipc/ → security::access_control::check_access() DIRECT (v6)	CRITIQUE
15	capability_bridge supprimé — toute référence = bug v6	CRITIQUE
16	security/capability/verify() = point d'entrée unique (CI grep)	CRITIQUE
17	SECURITY_READY flag atomique — APs spin-wait avant IPC	CRITIQUE
18	verify() constant-time — pas d'early return révélateur	HAUTE
19	VMA SignalTcb = VM_DONTCOPY | VM_DONTEXPAND	HAUTE
20	DMA IRQ : wakeup APRÈS libération de tout lock DMA	CRITIQUE
21	Data=Ordered FS : données physiques AVANT journal	CRITIQUE
22	Incompat flags ext4plus obligatoires (EXO_BLAKE3|DELAYED|REFLINK)	CRITIQUE
23	SYSRET : vérif RCX canonique — SIGSEGV si non-canonique	HAUTE
24	EmergencyPool : Err() jamais expect() — 256 slots min	CRITIQUE
25	// SAFETY: obligatoire avant tout unsafe{}	CRITIQUE

ExoFS Reference v3 est indépendant — ne pas mélanger ces règles avec celles de ce document.
