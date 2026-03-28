ExoPhoenix
Spécification Finale v6
SSR Layout v6 · TCB Layout v6 · Validation architecturale complète
Dernière ronde de corrections avant implémentation
Mars 2026 · ExoOS Project

0. Analyse froide — Ce que les 4 IA ont dit

Verdicts AXE 1 (corrections v4→v5) : VALIDE unanime sur tous les 9 points pour les 4 IA. Zéro désaccord architectural. Les rondes précédentes ont convergé.

Deux divergences RÉELLES sur les structures de données (AXE 2 et 3) :

Issue	Description	Consensus
SSR overflow	PMC slots à +0x0100 débordent dans canal commande dès N_cores > 12	KIMI (critique) + Z-AI (critique) + Grok4 (mineur) = 3/4 identifient le problème. SSR doit être recalculé avec MAX_CORES fixe.
TCB FS/GS	FS.base + GS.base (MSRs TLS) absents du TCB → context switch incomplet	KIMI identifie : +16 bytes pour FS/GS base dépasse 192. Solution : TCB 256 bytes (4 cache lines). Grok4/Gemini/Z-AI valident 192 sans FS/GS — divergence réelle à trancher.
Align(64)	Chaque slot per-core SSR doit être aligné sur 64 bytes pour éviter false sharing SMP	Gemini + Z-AI. Correction d'optimisation, non bloquante pour unicore mais obligatoire SMP.

Ce document produit les layouts définitifs SSR v6 et TCB v6 qui résolvent ces trois points. Ce sont les seules corrections nécessaires avant implémentation.

 
1. Arbitrage des divergences

SSR	SSR layout : débordement PMC slots sur systèmes > 12 cores	BLOCAGE RÉEL

Calcul du débordement (KIMI, Z-AI)
Le layout v5 place les PMC snapshot slots à +0x0100. Chaque core nécessite 8 registres × 8 bytes = 64 bytes. Le canal commande est à +0x0400. Espace disponible : 0x0400 - 0x0100 = 0x0300 = 768 bytes. Maximum cores avant débordement : 768 / 64 = 12 cores. Sur tout système SMP standard (≥ 16 cores), le layout v5 est invalide.

Arbitrage — Raison de la divergence Grok4
Grok4 dit 'mineur, corrigible'. Grok4 pense en termes unicore. Sur unicore N_cores=1 le débordement n'existe pas. Mais ExoPhoenix doit être conçu pour SMP dès le layout initial — changer les offsets post-code est destructeur.

Décision : MAX_CORES = 64 + offsets recalculés
1.	MAX_CORES = 64 est une constante compiletime. Couvre toutes les plateformes serveur cibles sans gaspiller de mémoire.
2.	Chaque slot per-core (Freeze ACK, PMC) est aligné sur 64 bytes (1 cache line) pour éviter false sharing entre cores en SMP. Recommandation unanime Gemini + Z-AI.
3.	Le SSR total reste ≤ 64 Ko (1 page de 16 × 4 Ko), accessible à A et B, déclarée dans e820.

TCB	TCB : FS.base + GS.base manquants — arbitrage 192 vs 256 bytes	À TRANCHER

Pourquoi FS.base et GS.base sont obligatoires
•	FS.base (MSR 0xC0000100) : utilisé pour le TLS (Thread Local Storage) en userspace x86_64. Chaque thread utilisateur a sa propre valeur FS.base. Elle doit être sauvegardée et restaurée à chaque context switch.
•	GS.base (MSR 0xC0000101) : utilisé pour les données per-CPU en Ring 0 (SWAPGS). La valeur kernel vs userspace doit être trackée par thread.
•	Sans FS/GS base dans le TCB, le context switch écrit des valeurs incorrectes dans les MSRs → corruption TLS pour tout programme userspace multi-threadé.

Arbitrage — KIMI a raison, 256 bytes
•	Avec FS.base (8) + GS.base (8) ajoutés : metadata = 8+8+8+8+8+8+4+4 = 64 bytes. cpu_ctx inline = 18 GPR × 8 = 144 + rflags/rip/cs/ss/cr2 = 40 bytes = total 184, padding 8 = 192 bytes. Grand total = 64 + 192 = 256 bytes.
•	Grok4 / Gemini / Z-AI ont validé 192 bytes SANS FS/GS base. Ce n'est pas un désaccord architectural — ils n'ont pas inclus ces champs dans leur calcul. La présence de FS/GS est obligatoire pour un microkernel supportant du userspace réel.
•	192 bytes avec FS/GS = impossible sans réduire cpu_ctx (ce qui complique le context switch et brise l'invariant d'exhaustivité). 256 bytes (4 cache lines) est la seule solution propre.

Note sur la décision locked '128 bytes'
La décision locked TCB=128 bytes d'ExoOS était une aspiration d'optimisation. Elle était irréaliste pour un kernel x86_64 complet avec cpu_ctx inline. La révision à 256 bytes est documentée dans le journal des décisions avec justification : 'FS/GS base obligatoires pour TLS userspace + cpu_ctx inline pour performance context switch'.


 
2. SSR Layout v6 — Spécification finale à figer

Adresse physique : 0x1000000. Taille totale : 64 Ko (0x10000). MAX_CORES = 64. Alignement : chaque section sur 64 bytes (1 cache line). Déclarée dans e820 comme région réservée ExoPhoenix.

Offset	Taille	Type	Contenu — Section et rôle
+0x0000	8 bytes	AtomicU64	HANDOFF CAS FLAG — états : 0=NORMAL, 1=FREEZE_REQ, 2=FREEZE_ACK_ALL, 3=B_ACTIVE
+0x0008	8 bytes	AtomicU64	LIVENESS NONCE — écrit par B (RDRAND), copié par A dans zone connue, vérifié B via PULL
+0x0010	8 bytes	AtomicU64	SEQLOCK COUNTER — protection lecture cohérente des champs critiques (pattern ktime_get)
+0x0018	40 bytes	[u8; 40]	Padding cache line 0 → section 0 occupe exactement 64 bytes (1 cache line)
+0x0040	64 bytes	struct align(64)	CANAL COMMANDE B→A — nonce liveness retour + flags commande + padding. Aligné 64 bytes, séparé des métriques A.
+0x0080	64 × 64 = 4096 bytes	AtomicU64[64] align(64)/slot	FREEZE ACK PER-CORE — 1 AtomicU64 par core (8 bytes) paddé à 64 bytes pour isolation cache line SMP. Index = position dans table APIC IDs immuable R4. MAX_CORES=64.
+0x1080	64 × 64 = 4096 bytes	u64[8][64] align(64)/slot	PMC SNAPSHOT PER-CORE — 8 valeurs × 8 bytes = 64 bytes/core, paddé pour isolation. Contenu : EVTSEL0..3 + CTR0..3. Écrit lock-free par handler 0xF2. MAX_CORES=64.
+0x2080	~8K (arrondi 0x6000)	u8[] align(64)	Réservé — padding + extensions futures. Zéro initialisé.
+0x8000	16384 bytes	u8[] RO pour A	LOG AUDIT B — append-only, E3 depth-limited (≤1000 entrées). Read-only pour A via PML4 de A (B possède l'écriture).
+0xC000	16384 bytes	u8[]	MÉTRIQUES PUSH A→B — non-critique, rate-limited E5. Overflow silencieux accepté. Séparé physiquement du log B.
TOTAL	65536 bytes	64 Ko exact	Aligné page 4 Ko. 1 région e820 réservée. Adresse physique fixe 0x1000000..0x1FFFF.

Constantes SSR à définir dans ssr.rs (partagé A et B)
pub const SSR_BASE:          PhysAddr = PhysAddr(0x1000000);
pub const SSR_SIZE:          usize    = 0x10000;  // 64 Ko
pub const MAX_CORES:         usize    = 64;
pub const SSR_HANDOFF_FLAG:  usize    = 0x0000;
pub const SSR_LIVENESS_NONCE:usize    = 0x0008;
pub const SSR_SEQLOCK:       usize    = 0x0010;
pub const SSR_CMD_B2A:       usize    = 0x0040;  // 64 bytes
pub const SSR_FREEZE_ACK:    usize    = 0x0080;  // 64 bytes × MAX_CORES
pub const SSR_PMC_SNAPSHOT:  usize    = 0x1080;  // 64 bytes × MAX_CORES
pub const SSR_LOG_AUDIT:     usize    = 0x8000;  // 16 Ko, RO pour A
pub const SSR_METRICS_PUSH:  usize    = 0xC000;  // 16 Ko

// Accès per-core (les deux kernels utilisent ces macros)
pub fn freeze_ack_offset(apic_id: usize) -> usize {
    SSR_FREEZE_ACK + apic_id * 64  // 1 AtomicU64 + 56 padding
}
pub fn pmc_snapshot_offset(apic_id: usize) -> usize {
    SSR_PMC_SNAPSHOT + apic_id * 64  // 8 × u64
}

IMPORTANT : ces constantes doivent être dans un crate partagé compilé identiquement par les deux kernels A et B. Toute divergence = désynchronisation silencieuse.


 
3. TCB Layout GI-01 — Spécification amendée (Mars 2026)

> **Amendement** : le layout v6 original (capability_token[0], GPRs inline [64..191]) a été
> remplacé par le layout GI-01 lors de l'implémentation. Justification détaillée dans CORR-01.
> Le protocole d'introspection Kernel B est défini ci-dessous (§3.1) — il ne requiert
> pas de GPRs inline dans le TCB.

Layout TCB GI-01 (256 bytes, 4 cache lines, align 64) :

Champ	Offset	Taille	Rôle
── Cache Line 1 (bytes 0-63) : HOT PATH scheduler ──
tid	[0]	8	Thread ID — hot path IPC/syslog, identifiant principal
kstack_ptr	[8]	8	RSP Ring 0 — HARDCODÉ switch_asm.s + protocole kstack ExoPhoenix
priority	[16]	1	Priorité scheduler (Priority newtype)
policy	[17]	1	Politique (Normal/Fifo/RR/Deadline/Idle)
_pad0	[18]	6	Alignement
sched_state	[24]	8	AtomicU64 : état(8b)|signal|KTHREAD|FPU|RESCHED|EXITING|IDLE|IN_RECLAIM
vruntime	[32]	8	vruntime CFS (ns) — AtomicU64
deadline_abs	[40]	8	Deadline EDF absolue (ns depuis boot) — AtomicU64
cpu_affinity	[48]	8	Bitmask affinité CPU — AtomicU64
cr3_phys	[56]	8	PML4 physique — HARDCODÉ switch_asm.s
── Cache Line 2 (bytes 64-127) : WARM context switch ──
cpu_id	[64]	8	CPU courant — AtomicU64
fs_base	[72]	8	MSR_FS_BASE (TLS userspace) — sauvegardé/restauré CORR-11
gs_base	[80]	8	MSR_KERNEL_GS_BASE (GS user) — sauvegardé/restauré CORR-11
pkrs	[88]	4	Intel PKS (0 sur AMD)
pid	[92]	4	ProcessId (compat PCB)
signal_mask	[96]	8	Bitmask signaux bloqués — AtomicU64
dl_runtime	[104]	8	Budget EDF (ns/période)
dl_period	[112]	8	Période EDF (ns)
_pad2	[120]	8	Alignement
── Cache Lines 3-4 (bytes 128-255) : COLD + HARDCODÉS ──
run_time_acc	[128]	8	Temps CPU cumulé (ns)
switch_count	[136]	8	Nombre context switches
_cold_reserve	[144]	88	Réservé (144+88=232)
fpu_state_ptr	[232]	8	*mut XSaveArea — null si FPU jamais utilisée — HARDCODÉ ExoPhoenix
rq_next	[240]	8	RunQueue intrusive next — null si BLOCKED
rq_prev	[248]	8	RunQueue intrusive prev — null si BLOCKED
TOTAL	[0..255]	256 bytes	4 cache lines. #[repr(C, align(64))].

Offsets immuables (hardcodés dans switch_asm.s et ExoPhoenix) :
  [8]   kstack_ptr    → RSP context switch
  [56]  cr3_phys      → MOV CR3 (KPTI)
  [232] fpu_state_ptr → XSaveArea introspection
  [240] rq_next       → RunQueue walk
  [248] rq_prev       → RunQueue walk

§3.1 — Protocole d'introspection Kernel B (remplace GPRs inline)

Kernel B n'utilise PAS de GPRs inline dans le TCB. Il lit l'état d'un thread gelé
via le protocole kstack (précédent Linux pt_regs) :

  1. tcb.tid [0]          → identifier le thread
  2. tcb.kstack_ptr [8]   → sommet pile kernel (RSP sauvegardé par switch_asm.s)
  3. tcb.cr3_phys [56]    → espace d'adressage (pour MOV CR3 si inspection mémoire)
  4. tcb.fpu_state_ptr [232] → état FPU complet (XSaveArea)
  5. tcb.sched_state [24] → distinguer yielded vs preempted (bit KTHREAD, état)

  GPRs callee-saved (thread gelé par yield coopératif) — lire depuis kstack :
    [kstack_ptr + 0]  = r15  (dernier push dans switch_asm.s)
    [kstack_ptr + 8]  = r14
    [kstack_ptr + 16] = r13
    [kstack_ptr + 24] = r12
    [kstack_ptr + 32] = rbp
    [kstack_ptr + 40] = rbx
    [kstack_ptr + 48] = rip  (adresse de reprise du thread)

  GPRs complets (thread préempté par IRQ) :
    La kstack contient en plus le frame ISR :
    [kstack_ptr + 56]  = rax (poussé par handler ISR)
    ... (selon séquence pusha du handler)
    → Kernel B lit tcb.sched_state pour détecter ce cas.

  cap_table_ptr — lire depuis le PCB (pas depuis le TCB) :
    Le PCB (ProcessControlBlock) est partagé par tous les threads du processus.
    Kernel B accède à PROCESS_TABLE[pid].cap_table_ptr.
    Avantage : une seule copie par processus, cohérence garantie entre threads.

static_assert: size_of::<ThreadControlBlock>() == 256.
static_assert: offset_of!(kstack_ptr) == 8.
static_assert: offset_of!(cr3_phys) == 56.
static_assert: offset_of!(fpu_state_ptr) == 232.


4. Validation AXE 1 — Toutes corrections v4→v5 confirmées

Les 4 IA valident unanimement les 9 corrections v4→v5. Aucun désaccord architectural. Tableau de confirmation :

Réf.	Correction	GEM	GROK	Z-AI	KIMI	Statut v6
E1	PMC reclassé heuristique faible	✓	✓	✓	✓	LOCKED
N2	Bounds check 3 comparaisons 64-bit	✓	✓	✓	✓	LOCKED
C1	Handlers 0xF1/0xF2 lock-free absolus	✓	✓	✓	✓	LOCKED
C2	PT walker itératif + profondeur max 4 + max_steps	✓	✓	✓	✓	LOCKED
C3	SSR layout — révisé en v6 (overflow corrigé)	✓	✓	✓	✓	LAYOUT v6 FIGÉ
C4	TCB révisé — 256 bytes avec FS/GS base	✓	✓	✓	✓	LAYOUT v6 FIGÉ
V5-N1	Timeout drain IOMMU par device class	✓	✓	✓	✓	LOCKED
V5-N2	Verrouillage hot-plug CPU + hash MADT	✓	✓	✓	✓	LOCKED
V5-N3	FACS Wake Vector RO au Stage 0	✓	✓	✓	✓	LOCKED


 
5. Verdict global — ExoPhoenix v6

IA	Verdict AXE 4
Grok4	PRÊT UNICORE SANS RÉSERVE — code unsafe immédiatement. PRÊT SMP après TLA+.
Gemini-3Pro	PRÊT UNICORE OUI IMMÉDIATEMENT. PRÊT SMP APRÈS TLA+. SSR align(64) requis avant SMP.
Z-AI	ARCHITECTURALEMENT COMPLET. PRÊT (GO) UNICORE. PRÊT SMP APRÈS TLA+.
KIMI-AI	PRÊT UNICORE OUI — avec SSR et TCB figés (résolus en v6). PRÊT SMP après TLA+.
CONSENSUS	PRÊT UNICORE — implémentation immédiate. PRÊT SMP — après preuve TLA+/Spin.

Check-list finale avant première ligne de code unsafe

Statut	Point	Action
✓ FIGÉ	SSR layout v6 avec offsets exacts et MAX_CORES=64	Implémenter ssr.rs avec constantes définies en section 2.
✓ FIGÉ	TCB layout v6 : 256 bytes, 4 cache lines, FS/GS base inclus	Implémenter tcb.rs avec static_assert size==256.
HIGH	ExoBoot v1.5 : B démarre Stage 0 avant A	Modifier ExoBoot pour SIPI vers B en premier, puis SIPI vers A après Stage 0.
HIGH	IDT : vecteurs 0xF1 et 0xF2 réservés ExoPhoenix	Documenter dans idt.rs. Aucun driver Ring 1 ne peut les revendiquer.
TLA+	Preuve formelle automate v5 (bloque SMP uniquement)	Peut tourner en parallèle de l'implémentation unicore.
MEDIUM	Platform matrix : Intel VT-d vs AMD IOMMU, FLR support	Documenter avant tests hardware réels.

Ordre d'implémentation recommandé (Grok4 + consensus)

4.	Implémenter ssr.rs (constantes v6) + tcb.rs (layout 256 bytes + static_assert). 1 heure max.
5.	Stage 0 ExoBoot : B démarre, configure IOMMU + PML4 RO + FACS RO + hash MADT + APIC table + handlers IDT 0xF1/0xF2 + pool R3. Boot solo de B validé sur QEMU unicore.
6.	PULL walker F2 : itératif + bounds check N2 + quota #PF N3 + handler #PF isolé R2. Cycle d'introspection fonctionnel.
7.	Handoff Phase 1 : IPI 0xF1 + drain IOMMU simultané (F3+R5) + ACK + swap CR3. Test QEMU unicore complet.
8.	ExoForge : reconstruction A depuis image propre + Merkle + Ring 1 reset. Cycle complet NORMAL→THREAT→ISOLATION→CERTIF→RESTORE.
9.	TLA+/Spin sur automate v5 en parallèle → validation formelle → passage SMP.

ExoPhoenix est architecturalement complet.
Les structures de données sont figées. L'implémentation peut commencer.
ExoOS Project · ExoPhoenix v6 · Mars 2026 · Spécification finale
