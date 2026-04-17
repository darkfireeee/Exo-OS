TIMEKEEPING & SCHEDULER
Arborescences · Règles de création · Erreurs silencieuses · Double analyse
ExoOS — Phase 2b  |  Timekeeping dédié + Scheduler corrigé  |  Bare-metal ready
🔬  Document produit après double analyse (PASSE 1 + PASSE 2). La PASSE 2 a corrigé 6 hypothèses fausses de la PASSE 1. Les corrections sont intégrées dans chaque section.

Module	Couche	État actuel	Objectif après ce doc
arch/x86_64/time/ (timekeeping)	Ring 0 — Couche 0	❌ Inexistant — ERR-01 actif	Module complet, ktime_get() seqlock, calibration HPET window réelle
scheduler/timer/clock.rs	Ring 0 — consommateur	⚠️ TSC 3GHz fallback	Utilise ktime_get() depuis timekeeping — ne calibre plus lui-même
scheduler/ (global)	Ring 0	75% — SWAPGS manquant	Arborescence complète avec dépendances clarifiées
 
1 — Diagnostic : pourquoi la correction actuelle échoue sur bare-metal
1.1 — Le bug racine : itérations ≠ temps
🔴  La correction actuelle substitue un délai temporel par une limite d'itérations fixes. Ce n'est pas du tout la même chose. Sur QEMU, ça fonctionne par accident car les MMIO reads sont lents (~50µs). Sur bare-metal, ils sont 500× plus rapides (~100ns) et la fenêtre mesurée n'atteint jamais 10ms.

// ❌ ÉTAT ACTUEL — fonctionne sur QEMU par accident
const MAX_MEASURE_ITERS: usize = 500;
let mut count = 0;
while count < MAX_MEASURE_ITERS {
    let hpet_now = read_volatile(HPET_COUNTER);  // ~50µs QEMU | ~100ns bare-metal
    count += 1;
}
// Sur QEMU   : 500 × 50µs  = 25ms  → dépasse 10ms cible ✅ par accident
// Bare-metal : 500 × 100ns = 50µs  → jamais 10ms ❌ → retourne false
// Conséquence bare-metal : fallback CPUID nominal → précision ±30%
 
// ✅ ÉTAT REQUIS — condition temporelle réelle
let hpet_freq   = hpet::frequency_hz();         // depuis ACPI HPET table
let target_ticks = hpet_freq / 1000;            // 1ms en ticks HPET
let hpet_start   = hpet::read_counter();
let tsc_start    = rdtsc();
loop {
    // Loop condition = HPET ticks écoulés (temps réel)
    // Jamais une limite d'itérations
    if hpet::read_counter().wrapping_sub(hpet_start) >= target_ticks { break; }
}
let tsc_delta  = rdtsc() - tsc_start;
let tsc_hz     = tsc_delta * 1000;              // extrapolé à 1 seconde
// Bare-metal : loop tourne ~1ms de vrai temps → TSC delta précis ✅

1.2 — Impact selon le contexte
Contexte	État avec correction actuelle	État avec timekeeping complet
QEMU / développement	✅ Suffisant — fonctionne par accident	✅ Fonctionne mieux (précision garantie)
Bare-metal, charge légère	⚠️ TSC ±30% selon CPU — scheduler imprécis	✅ TSC calibré <0.1% — scheduling correct
Bare-metal, Turbo Boost actif	❌ Turbo fausse le nominal — délais 2× trop courts	✅ TSC invariant mesuré sur fréquence base
Bare-metal, multi-socket NUMA	❌ TSC offset inter-socket non corrigé	✅ RDTSCP + correction offset par cœur
Audio temps-réel (<1ms)	❌ Jitter inacceptable	✅ ktime_get() seqlock <100ns
Production réelle	❌ Non utilisable	✅ Prêt pour production

2 — Arborescence : module timekeeping dédié
📋  RÈGLE ARCH-TIME-01 : Le timekeeping est un module SÉPARÉ du scheduler. Le scheduler est un CONSOMMATEUR de ktime_get(). Il ne calibre jamais lui-même le TSC. Cette séparation évite la dépendance circulaire : calibration → timer → scheduler → calibration.

kernel/src/arch/x86_64/time/
├── mod.rs                   // Exports publics : ktime_get(), ktime_get_ns(),
│                            // monotonic_ns(), wall_time_ns()
│                            // Init : time::init(acpi_info) appelé en early_init étape 6b
│
├── ktime.rs                 // Type ktime_t + seqlock pour cohérence ISR
│   // RÈGLE TIME-SEQLOCK-01 : ktime_get() utilise seqlock
│   // ISR peut appeler ktime_get() sans lock — retry si write en cours
│   // Jamais de Mutex ici — ktime_get() doit être wait-free
│
├── sources/                 // Clock sources — abstraction commune
│   ├── mod.rs               // ClockSource trait + registry
│   │   // Chaque source déclare : name, rating (qualité 0-500), read() -> u64
│   │   // Sélection automatique : rating le plus haut disponible
│   ├── tsc.rs               // TSC — rating 400 si invariant, 50 si non-invariant
│   │   // rdtsc() + rdtscp() (avec coreid)
│   │   // check_tsc_invariant() via CPUID leaf 0x80000007 bit 8
│   │   // check_tsc_reliable() : vérifie cohérence entre plusieurs lectures
│   ├── hpet.rs              // HPET — rating 300
│   │   // read_counter() via MMIO UC (déjà mappé avec PAGE_FLAGS_MMIO)
│   │   // frequency_hz() depuis ACPI HPET table (champ period)
│   │   // handle_32bit_wrap() : wrapping_sub sur compteur 32-bit
│   ├── pm_timer.rs          // ACPI PM Timer — rating 200
│   │   // Port I/O ou MMIO selon FADT (PMT_type bit)
│   │   // 24-bit ou 32-bit selon FADT (TMR_VAL_EXT bit)
│   │   // frequency_hz() = 3_579_545 Hz (fixe, défini par spec ACPI)
│   └── pit.rs               // PIT canal 0 — rating 50 (fallback ultime)
│       // Fréquence : 1_193_182 Hz (fixe)
│       // RÈGLE PIT-QEMU-01 : PIT ne fonctionne pas sur QEMU TCG en busy-wait
│       // PIT utilisé UNIQUEMENT si HPET et PM Timer tous les deux absents
│
├── calibration/             // Calibration TSC sur fenêtre temporelle réelle
│   ├── mod.rs               // Orchestration + sélection source de référence
│   │   // Ordre : HPET → PM Timer → CPUID 0x15 → PIT → fallback nominal
│   ├── window.rs            // Mesure sur fenêtre réelle (HPET ou PM Timer)
│   │   // RÈGLE CAL-WINDOW-01 : loop condition = ticks écoulés, jamais itérations
│   │   // RÈGLE CAL-CLI-01 : cli/sti par sample de 1ms MAX (pas 10ms global)
│   │   // multi_sample() : 10 mesures × 1ms + outlier rejection (IQR)
│   ├── cpuid_nominal.rs     // CPUID 0x15 (crystal ratio) — TSC Hz depuis CPU
│   │   // Disponible sur Skylake+ — précis à ~0.01% mais fixe
│   │   // cpuid_tsc_hz() : retourne Some(hz) si leaf 0x15 disponible
│   └── validation.rs        // Validation croisée des résultats
│       // validate_calibration(tsc_hz) : cohérent avec CPUID nominal ?
│       // Si écart > 10% : warning + re-mesure
│
├── drift/                   // Correction de dérive TSC long terme
│   ├── mod.rs               // Exports : drift_correction_init(), apply_correction(ns)
│   ├── periodic.rs          // Thread de recalibration périodique
│   │   // RÈGLE DRIFT-PREEMPT-01 : preempt_disable() AVANT la mesure
│   │   // RÈGLE DRIFT-CIRCULAR-01 : NE PAS appeler ktime_get() ici
│   │   //   → lire HPET et TSC directement
│   │   // Période : toutes les 30 secondes en idle, 5 secondes sous charge
│   └── pll.rs               // Software PLL pour correction lissée
│       // Évite les sauts de temps brutaux lors de la correction
│       // adj_freq : ±500 ppm maximum par correction
│
└── percpu/                  // Données per-CPU pour TSC
    ├── mod.rs               // Per-CPU : tsc_offset, last_tsc, last_ktime
    └── sync.rs              // Synchronisation TSC inter-cœurs au boot SMP
        // RÈGLE TSC-SYNC-01 : TSC offset de chaque AP mesuré depuis BSP
        // RÈGLE TSC-RDTSCP-01 : ktime_get() utilise RDTSCP (pas RDTSC)
        //   → RDTSCP garantit que le CPU ne réordonne pas les lectures

3 — Arborescence : scheduler mis à jour
📋  La Phase 2 est à 75% — SWAPGS manquant, calibration TSC 3GHz fallback actif. Cette arborescence intègre les TODOs bloquants identifiés dans PHASE2_SCHEDULER_IPC.md et supprime la dépendance du scheduler sur la calibration TSC.

kernel/src/scheduler/
├── mod.rs                   // init(params: SchedInitParams) — 11 étapes dans l'ordre
│   // RÈGLE SCHED-INIT-01 : scheduler::init() appelé APRÈS time::init()
│   // SchedInitParams ne contient PLUS tsc_hz — vient de ktime_get()
│
├── core/
│   ├── task.rs              // TCB 128 bytes — 2 cache lines exactes
│   │   // static_assert!(size_of::<TCB>() == 128) OBLIGATOIRE
│   │   // static_assert!(align_of::<TCB>() == 64) OBLIGATOIRE
│   │   // _pad2 (8 bytes) = anciens ThreadAiState supprimés → disponibles
│   ├── runqueue.rs          // RunQueue par CPU — intrusive list, 0 alloc ISR
│   │   // RÈGLE SCHED-08 : WaitNode depuis EmergencyPool en ISR
│   ├── pick_next.rs         // pick_next_task() — O(1) hot path 100-150 cycles
│   ├── switch.rs            // Context switch orchestration
│   │   // RÈGLE SWITCH-01 : check_signal_pending() = lecture seule, jamais livraison
│   │   // RÈGLE SWITCH-02 : FPU sauvée AVANT switch, marquée APRÈS
│   └── preempt.rs           // preempt_disable/enable + compteur
│       // RÈGLE PREEMPT-01 : chaque disable DOIT avoir un enable symétrique
│       // preempt_count() == 0 requis avant tout sleep()
│
├── asm/
│   └── switch_asm.s         // Context switch assembleur
│       // Sauvegarde : rbx, rbp, r12-r15, rsp, MXCSR, x87 FCW
│       // CR3 switché AVANT restauration registres (KPTI correct)
│       // ✅ TODO BLOQUANT SMP : SWAPGS à l'entrée ET à la sortie Ring 0
│       //   SWAPGS entrée : kernel GS = percpu base
│       //   SWAPGS sortie : user GS = TLS thread
│       //   Sans ça : gs.base pointe vers données user en mode kernel
│
├── fpu/
│   ├── save_restore.rs      // xsave64/xrstor64 ou fxsave64/fxrstor64
│   ├── state.rs             // FpuState 512B aligné 64B, XSAVE_AREA_SIZE
│   └── lazy.rs              // CR0.TS=1 au boot, exception #NM au 1er accès
│
├── timer/                   // Consommateur de timekeeping — NE calibre plus
│   ├── clock.rs             // ⚠️ REMPLACER : était la source TSC
│   │   // APRÈS correction : délègue à arch::x86_64::time::ktime_get()
│   │   // scheduler_now_ns() → ktime_get_ns() — une ligne
│   ├── tick.rs              // Tick handler HZ=1000 — appelé depuis LAPIC timer ISR
│   │   // RÈGLE TICK-01 : tick handler < 5µs — pas d'alloc, pas de lock global
│   ├── hrtimer.rs           // High-resolution timers — utilise ktime_get_ns()
│   │   // RÈGLE HRTIMER-01 : expirations vérifiées dans tick handler
│   │   //   ET dans pick_next_task() pour les threads Deadline
│   └── deadline_timer.rs    // EDF deadline timers pour politique Deadline
│
├── policies/
│   ├── cfs.rs               // CFS vruntime — DÉTERMINISTE (modules IA supprimés)
│   │   // vruntime += delta_ns * NICE_0_WEIGHT / weight
│   │   // rbtree par vruntime minimal (intrusive)
│   ├── rt.rs                // FIFO + RoundRobin — priorités 1-99
│   └── deadline.rs          // EDF — runtime/deadline/period en ns
│
├── smp/
│   ├── topology.rs          // Topologie CPU : cœurs, threads, nœuds NUMA
│   │   // ✅ TODO BLOQUANT SMP : rdmsr(IA32_GS_BASE) pour current_thread_raw()
│   │   // Actuellement : CURRENT_THREAD_PER_CPU[0] (mono-CPU seulement)
│   ├── ipi.rs               // Inter-Processor Interrupts (TLB shootdown, reschedule)
│   └── percpu.rs            // Données per-CPU : current_thread, runqueue, stats
│
├── sync/
│   ├── wait_queue.rs        // WaitQueue — nœuds depuis EmergencyPool (SCHED-08)
│   │   // wake_up() : appelable depuis ISR — pas de lock scheduler
│   └── condvar.rs           // CondVar kernel — connaît le scheduler
│
└── energy/
    └── c_states.rs          // C-states ACPI — idle loop avec HLT/MWAIT

4 — Algorithme de calibration correct (bare-metal ready)
4.1 — Calibration HPET window avec multi-sample
⚠️  RÈGLE CAL-CLI-01 (issue PASSE 2) : Ne jamais désactiver les IRQ pendant plus de 1ms. La calibration utilise 10 samples de 1ms avec cli/sti par sample. Pas un seul sample de 10ms — cela perdrait des IRQs.

// kernel/src/arch/x86_64/time/calibration/window.rs
 
const SAMPLE_DURATION_TICKS: u64 = 0;  // calculé dynamiquement depuis hpet_freq
const N_SAMPLES: usize = 10;
const OUTLIER_MARGIN: u64 = 5;  // rejeter si écart > 5% de la médiane
 
/// Mesure le TSC Hz en utilisant le HPET comme référence temporelle.
/// RÈGLE CAL-WINDOW-01 : loop condition = ticks HPET, jamais itérations fixes.
pub fn calibrate_tsc_via_hpet(hpet_freq_hz: u64) -> Option<u64> {
    // 1ms de ticks HPET
    let target_ticks = hpet_freq_hz / 1000;
    let mut samples = [0u64; N_SAMPLES];
 
    for i in 0..N_SAMPLES {
        // RÈGLE CAL-CLI-01 : cli/sti par sample de 1ms MAX
        let flags = unsafe { x86_64::cli_save() };
 
        let hpet_start = hpet::read_counter();   // UC MMIO — ordre garanti
        let tsc_start  = unsafe { core::arch::x86_64::_rdtsc() };
 
        // ✅ Loop condition = HPET delta ticks (temps réel)
        // ❌ PAS : while iter < MAX_ITERS
        loop {
            // Barrière mémoire : empêche réordonnancement RDTSC / HPET read
            core::sync::atomic::fence(Ordering::SeqCst);
            let hpet_now = hpet::read_counter();
            // wrapping_sub gère le rollover 32-bit (safe pour window de 1ms)
            if hpet_now.wrapping_sub(hpet_start) >= target_ticks { break; }
        }
 
        // RDTSCP : garantit que le CPU n'a pas migré + sérialise la lecture
        let (tsc_end, _coreid) = unsafe { core::arch::x86_64::__rdtscp() };
 
        unsafe { x86_64::sti_restore(flags); }
 
        let tsc_delta = tsc_end.wrapping_sub(tsc_start);
        // Extrapolation : 1ms → 1 seconde
        samples[i] = tsc_delta.saturating_mul(1000);
    }
 
    // Rejet des outliers par IQR (interquartile range)
    samples.sort_unstable();
    let q1  = samples[N_SAMPLES / 4];
    let q3  = samples[3 * N_SAMPLES / 4];
    let iqr = q3.saturating_sub(q1);
    // Garder uniquement les samples dans [q1 - 1.5×IQR, q3 + 1.5×IQR]
    let valid: Vec<u64> = samples.iter()
        .filter(|&&s| s >= q1.saturating_sub(iqr + iqr/2)
                   && s <= q3.saturating_add(iqr + iqr/2))
        .copied().collect();
 
    if valid.is_empty() { return None; }
    // Moyenne des samples valides
    let tsc_hz = valid.iter().sum::<u64>() / valid.len() as u64;
    // Validation croisée avec CPUID 0x15 si disponible
    validation::cross_check(tsc_hz)
}

4.2 — ktime_get() avec seqlock (ISR-safe)
🔴  ERREUR SILENCIEUSE TIME-01 (issue PASSE 2) : Si ktime est une struct {secs, nanos} mise à jour en deux écritures, une ISR entre les deux writes voit un temps incohérent. Le seqlock résout ça : le lecteur retire si un write est en cours, sans aucun lock.

// kernel/src/arch/x86_64/time/ktime.rs
 
/// Horloge monotone globale — mise à jour par le thread de drift correction
/// RÈGLE TIME-SEQLOCK-01 : toujours lire via ktime_get() — jamais accès direct
static KTIME_STATE: KtimeState = KtimeState::new();
 
#[repr(C, align(64))]  // Cache line entière pour éviter false sharing
struct KtimeState {
    seq:       AtomicU64,    // Seqlock counter : pair = stable, impair = en écriture
    tsc_base:  AtomicU64,    // TSC lu au dernier point d'ancrage
    ns_base:   AtomicU64,    // Nanosecondes au dernier point d'ancrage
    tsc_hz:    AtomicU64,    // TSC Hz actuel (mis à jour par drift correction)
    _pad:      [u8; 32],     // Compléter la cache line
}
 
/// Lecture ISR-safe — wait-free, pas de lock, pas d'alloc
/// Cycles typiques : 15-50 cycles sur CPU moderne
#[inline(always)]
pub fn ktime_get_ns() -> u64 {
    loop {
        // 1. Lire le seq AVANT — pair = état stable
        let seq1 = KTIME_STATE.seq.load(Ordering::Acquire);
        if seq1 & 1 != 0 {
            // Impair = write en cours → spin et retry
            core::hint::spin_loop();
            continue;
        }
        // 2. Lecture RDTSCP — ancré au cœur logique courant
        let (tsc_now, _coreid) = unsafe { core::arch::x86_64::__rdtscp() };
        // 3. Calcul nanoseconds depuis le dernier anchor
        let tsc_base = KTIME_STATE.tsc_base.load(Ordering::Acquire);
        let ns_base  = KTIME_STATE.ns_base.load(Ordering::Acquire);
        let tsc_hz   = KTIME_STATE.tsc_hz.load(Ordering::Acquire);
        // 4. Vérifier que le seq n'a pas changé (pas de write pendant notre lecture)
        let seq2 = KTIME_STATE.seq.load(Ordering::Acquire);
        if seq1 != seq2 {
            // Write est arrivé pendant notre lecture → retry
            continue;
        }
        // 5. Calcul final — sécurisé car seq stable
        let tsc_delta = tsc_now.wrapping_sub(tsc_base);
        // ns = tsc_delta * 1_000_000_000 / tsc_hz
        // ⚠️  Division entière : utiliser u128 pour éviter overflow
        let ns_delta = (tsc_delta as u128 * 1_000_000_000 / tsc_hz as u128) as u64;
        return ns_base.wrapping_add(ns_delta);
    }
}
 
/// Mise à jour de l'anchor (appelée par drift correction uniquement)
/// RÈGLE TIME-ANCHOR-01 : NE PAS appeler ktime_get_ns() ici
pub(crate) fn update_ktime_anchor(tsc_now: u64, ns_now: u64, tsc_hz: u64) {
    // seq impair = write en cours
    KTIME_STATE.seq.fetch_add(1, Ordering::Release);
    KTIME_STATE.tsc_base.store(tsc_now,  Ordering::Release);
    KTIME_STATE.ns_base .store(ns_now,   Ordering::Release);
    KTIME_STATE.tsc_hz  .store(tsc_hz,   Ordering::Release);
    // seq pair = état stable
    KTIME_STATE.seq.fetch_add(1, Ordering::Release);
}

4.3 — Fallback chain complet
Source	Rating	Fréquence	Disponible si	Limite principale
HPET	300	Variable (ACPI) typique : 14.318 MHz	ACPI HPET table présente MMIO UC mappé	Optionnel depuis ACPI 2.0 Absent sur certains systèmes
ACPI PM Timer	200	3.579545 MHz (fixe)	FADT.PM_TMR_BLK != 0	24-bit wraps/4.7s (géré) Port I/O plus lent que MMIO
CPUID leaf 0x15	150	Nominal fabricant (cristal + ratio)	Skylake+ (2015+) BIOS non buggé	Fixe — ne détecte pas drift Absent sur CPU anciens
TSC direct (CPUID 0x16)	100	Fréquence base CPU	Skylake+ uniquement	Approximation ±5% Pas un vrai calibrage
PIT canal 0	50	1.193182 MHz (fixe)	Toujours (héritage)	Ne fonctionne pas en QEMU TCG en busy-wait (bug connu)
Fallback 3GHz nominal	10	Fixe — valeur hard-codée	Toujours (last resort)	Erreur jusqu'à 30% Inacceptable en production

5 — Règles de création : court terme et long terme
5.1 — Règles architecturales (jamais violées)
// RÈGLE ARCH-TIME-01 : SÉPARATION scheduler / timekeeping
// scheduler/timer/clock.rs NE mesure PAS le TSC
// scheduler_now_ns() = ktime_get_ns()  ← une seule ligne
// Viole si : scheduler/timer/ contient rdtsc(), hpet::read_counter(), calibrate()
 
// RÈGLE ARCH-TIME-02 : ktime_get_ns() = primitive universelle
// Tout code qui a besoin de l'heure appelle ktime_get_ns()
// Jamais rdtsc() direct en dehors de arch/x86_64/time/
// Viole si : scheduler, IPC, ExoFS, syscalls appellent rdtsc() directement
 
// RÈGLE ARCH-TIME-03 : seqlock obligatoire pour toute clock globale
// Une clock globale mise à jour depuis un thread ET lue depuis une ISR
// DOIT utiliser seqlock — jamais Mutex (deadlock ISR), jamais RwLock
 
// RÈGLE ARCH-TIME-04 : les horloges ne régressent JAMAIS
// ktime_get_ns() retourne toujours >= valeur précédente
// Si drift correction abaisse tsc_hz, compenser via ns_base adjustment
// Jamais : KTIME_STATE.ns_base.store(valeur < ancienne_valeur)

5.2 — Règles de calibration
// RÈGLE CAL-WINDOW-01 : condition de sortie = temps réel HPET/PM Timer
// Jamais : while iter < MAX_ITERS
// Toujours : while hpet_now.wrapping_sub(hpet_start) < target_ticks
 
// RÈGLE CAL-CLI-01 : IRQ désactivées MAX 1ms par sample
// Jamais : cli() ... 10ms de mesure ... sti()
// Toujours : 10 samples × (cli, 1ms mesure, sti)
 
// RÈGLE CAL-RDTSCP-01 : toujours RDTSCP (pas RDTSC) en calibration
// RDTSCP serialise + fournit coreid — empêche réordonnancement CPU
// RDTSC peut être exécuté out-of-order → biais mesure
 
// RÈGLE CAL-VALIDATE-01 : validation croisée obligatoire
// Si HPET mesure 3.2 GHz et CPUID 0x15 dit 3.0 GHz → écart 6.7% → warning
// Seuil : écart > 10% = erreur, re-mesure obligatoire
// Seuil : écart > 20% = utiliser CPUID nominal (hardware suspect)
 
// RÈGLE CAL-FALLBACK-01 : dégrader gracieusement, toujours logguer
// Chaque fallback dans la chaîne DOIT émettre un log de niveau WARN
// Le log DOIT contenir : source utilisée, Hz mesuré, raison du fallback

5.3 — Règles de correction de dérive
// RÈGLE DRIFT-PREEMPT-01 : preempt_disable() pendant la mesure de dérive
// Sans ça : thread de recalibration préempté → mesure biaisée → correction fausse
// preempt_disable() avant lecture HPET+TSC, preempt_enable() après
 
// RÈGLE DRIFT-CIRCULAR-01 : thread de recalibration N'appelle PAS ktime_get_ns()
// ktime_get_ns() utilise tsc_hz que le thread est en train de mettre à jour
// → lire HPET et TSC directement via les fonctions de bas niveau
 
// RÈGLE DRIFT-PLL-01 : correction max ±500 ppm par recalibration
// Un saut brutal de tsc_hz provoque des discontinuités de temps applicatif
// Filtrage PLL : adj = clamp(measured_hz - current_hz, -500ppm, +500ppm)
// Convergence progressive sur plusieurs cycles si dérive importante
 
// RÈGLE DRIFT-MONOTONE-01 : ktime ne peut pas reculer
// Avant d'appeler update_ktime_anchor(tsc_now, ns_now, tsc_hz) :
// assert!(ns_now >= ns_base_actuel)
// Si ns_now < ns_base → ajuster ns_base uniquement si tsc_hz a BAISSÉ

5.4 — Règles SMP / per-CPU
// RÈGLE TSC-SYNC-01 : TSC offset de chaque AP mesuré depuis BSP au boot SMP
// Méthode : BSP envoie IPI à AP, AP lit TSC, envoie à BSP, BSP compare
// Stocker tsc_offset[cpu_id] dans per-CPU data
// ktime_get_ns() : tsc_delta = (rdtscp() - tsc_offset[coreid]) - tsc_base
 
// RÈGLE TSC-RDTSCP-01 : RDTSCP obligatoire dans ktime_get_ns() et calibration
// RDTSC n'est pas sérialisé — le CPU peut réordonner avant/après
// RDTSCP fournit aussi coreid → permet d'appliquer le bon tsc_offset
 
// RÈGLE SWAPGS-01 (bloquant SMP) : SWAPGS à l'entrée ET la sortie Ring 0
// Entrée : SWAPGS → kernel GS = percpu base → gs.current_thread valide
// Sortie : SWAPGS → user GS = TLS thread → accès TLS Ring 3 valide
// Sans ça : current_thread_raw() retourne une adresse Ring 3 → UB kernel

6 — Catalogue des erreurs silencieuses : timekeeping + scheduler
🔬  Ces erreurs ne crashent pas le système immédiatement. Elles produisent des comportements incorrects sous charge, sur hardware réel, ou après un uptime prolongé. Chacune a une correction précise issue de la double analyse.

ID	Module	Erreur silencieuse	Symptôme tardif	Correction (PASSE 2 validée)
TIME-01 (critique)	ktime.rs	Struct ktime mise à jour en 2 writes sans seqlock ISR lit un temps incohérent	Audio : clicks/pops Réseau : timestamps faux FS : order violation ExoFS	seqlock : seq impair pendant write lecteur retry si seq change Cache line alignée 64B
TIME-02 (critique)	calibration/ window.rs	Loop condition = MAX_ITERS Pas une fenêtre temporelle réelle Bare-metal : 50µs au lieu de 10ms	TSC Hz faux → scheduling drift Timeouts 10-30× trop courts/longs Invisible sur QEMU	Loop condition = hpet ticks wrapping_sub(hpet_start) >= target Jamais itérations fixes
TIME-03 (sérieux)	calibration/ window.rs	cli pendant 10ms (1 sample long) IRQs perdues pendant la mesure	LAPIC timer IRQ perdue Thread endormi 10ms ne se réveille plus I/O silencieusement retardée	10 samples × 1ms (cli/sti par sample) Perte max : 1ms d'IRQs par sample Outlier rejection sur 10 samples
TIME-04 (sérieux)	sources/tsc.rs	TSC non-invariant (pré-Nehalem) utilisé sans check CPUID	TSC change avec les C-states et le Turbo Boost → ktime régresse ou saute	CPUID 0x80000007 bit 8 = TSC Invariant flag Si absent → rating TSC = 50 utiliser HPET ou PM Timer
TIME-05 (sérieux)	percpu/sync.rs	TSC offset inter-sockets NUMA non mesuré ni corrigé	Sur 2-socket : ktime_get_ns() retourne des temps différents selon le CPU → race conditions ExoFS	RDTSCP → coreid tsc_offset[coreid] mesuré au boot SMP ktime = (tsc - offset[coreid] - base) * mult
TIME-06 (modéré)	drift/periodic.rs	Thread de recalibration préempté pendant sa propre mesure de dérive	Dérive calculée fausse Correction PLL oscille Temps légèrement erratique sous charge	preempt_disable() avant HPET+TSC read preempt_enable() après Mesure < 2ms pour limiter impact
TIME-07 (modéré)	drift/periodic.rs	Thread de recalibration appelle ktime_get_ns() (dépendance circulaire) ktime_get utilise tsc_hz en cours MAJ	Boucle de dépendance Valeur lue = valeur en cours de write → temps incohérent	Lire HPET et TSC directement Jamais ktime_get_ns() dans ce thread RÈGLE DRIFT-CIRCULAR-01
TIME-08 (modéré)	sources/hpet.rs	HPET 32-bit counter wraps toutes les ~300 secondes sans wrapping_sub	Après 5 minutes : temps régresse brutalement de 300s ExoFS : timestamps incohérents	TOUJOURS wrapping_sub pour delta Jamais soustraction directe u32::wrapping_sub garanti correct
TIME-09 (modéré)	sources/ pm_timer.rs	PM Timer 24-bit wraps/4.7s sans protection explicite	Rare mais possible si PM Timer lent → wrap pendant fenêtre de mesure → delta négatif → tsc_hz énorme	Détecter wrap : if end < start { end += 1<<24 } Ou utiliser PM Timer 32-bit si FADT.TMR_VAL_EXT
SCHED-01 (bloquant SMP)	asm/switch_asm.s	SWAPGS absent Kernel GS = user GS current_thread_raw() retourne ptr Ring3	current_thread pointe vers mémoire user Lecture → données aléatoires SMP : crash kernel silencieux puis panic	SWAPGS à l'entrée Ring0 (syscall/irq) SWAPGS à la sortie Ring0 PRIORITÉ MAXIMALE avant SMP
SCHED-02 (bloquant SMP)	smp/topology.rs	current_thread_raw() utilise CPU0 seul (mode mono-CPU) SMP : CPU1..N lisent le thread CPU0	CPU1 et CPU0 'exécutent' le même thread Double free de ressources Race conditions massives	rdmsr(IA32_GS_BASE) → percpu base → gs.current_thread par CPU Requiert SWAPGS correct (SCHED-01)
SCHED-03 (sérieux)	core/preempt.rs	preempt_enable() oublié après preempt_disable() dans chemin d'erreur	Scheduler jamais préempté Threads temps-réel bloqués Audio/réseau : starvation silencieuse	Rust RAII : PreemptGuard::new() et Drop Drop appelle preempt_enable() Panic si guard leaké (debug mode)
SCHED-04 (sérieux)	timer/tick.rs	Tick handler > 5µs Alloc ou lock global dans le tick handler	Jitter accumulé → drift d'horloge Audio : glitches progressifs Threads Deadline : deadlines manquées	static_assert: tick handler benchmark CI : mesure cycles dans tick handler Si > 500 cycles → fail CI
SCHED-05 (modéré)	timer/ hrtimer.rs	hrtimer utilisé avant calibration TSC ktime_get_ns() retourne 0 ou garbage pendant les premières ms du boot	Premiers hrtimers expirent instantanément Boot semblerait rapide mais threads RT démarrés trop tôt	time::init() AVANT scheduler::init() Assert dans hrtimer::init() : assert!(ktime_get_ns() > 0)

7 — Séquence d'initialisation corrigée (early_init.rs)
📋  La séquence boot actuelle (PHASE1_MEMOIRE.md) a l'étape 6 = init_tsc. Avec le module timekeeping dédié, cette étape est remplacée par time::init() qui orchestre tout. Les étapes suivantes utilisent ktime_get_ns() sans jamais calibrer elles-mêmes.

// kernel/src/arch/x86_64/boot/early_init.rs — SÉQUENCE CORRIGÉE
 
// Étape 1  '1' — init_cpu_features()      (SSE2, SYSCALL, XSAVE, TSC...)
// Étape 2  '2' — init_gdt_for_cpu()        (GDT per-CPU BSP)
// Étape 3  '3' — init_idt()                (IDT + load_idt)
// Étape 4       — TSS + IST stacks          (dans init_gdt_for_cpu)
// Étape 5  '5' — init_percpu_for_bsp()     (per-CPU data + GSBASE)
//               ↑ GSBASE requis pour ktime per-CPU data
 
// Étape 6  '6' — time::init(acpi_rsdp)    ← REMPLACE init_tsc() seul
//   time::init() fait dans l'ordre :
//   6.1 — Tenter HPET (si table ACPI HPET présente)
//         hpet::init_from_acpi(acpi_info)
//         [⚠️ HPET MMIO doit être mappé UC AVANT — fait en Phase 1 ✅]
//   6.2 — Tenter PM Timer si HPET absent (FADT.PM_TMR_BLK)
//   6.3 — Calibrer TSC via source disponible (HPET → PM → CPUID → fallback)
//         RÈGLE CAL-WINDOW-01 : loop condition = ticks réels
//         RÈGLE CAL-CLI-01 : 10 samples × 1ms
//   6.4 — Vérifier TSC invariant (CPUID 0x80000007 bit 8)
//   6.5 — Initialiser KtimeState avec seqlock
//   6.6 — Valider : assert!(ktime_get_ns() > 0)
//   6.7 — Émettre marqueur debug '6' sur port 0xE9 (comme avant)
 
// Étape 7  '7' — init_fpu_for_cpu()        (inchangé)
// Étape 8  '8' — detect_hypervisor()        (inchangé)
// Étape 9  '9' — ACPI init                  (inchangé — RSDP déjà parsé pour time::init)
// Étape 10 'a' — init_apic_system()          (inchangé)
// Étape 11 'b' — calibrate_lapic_timer()     (UTILISE ktime_get_ns())
//                ← plus de dépendance sur tsc_hz interne
// Étape 12 'c' — init_memory_integration()   (inchangé)
// Étape 13 'd' — init_syscall()              (inchangé)
// Étape 14 'e' — apply_mitigations_bsp()     (inchangé)
// Étape 15 'f' — Parse Multiboot2 / BootInfo (inchangé)
// Étape 16 'g' — SMP boot                    (REQUIERT SWAPGS — TODO)
//    Pour chaque AP :
//    — percpu::sync::measure_tsc_offset(bsp_tsc, ap_tsc)
//    — Stocker tsc_offset[ap_id] dans per-CPU data
//    → ktime_get_ns() sur AP utilise le bon offset
 
// Sortie attendue INCHANGÉE : XK12356789abcdefgZAIOK
// (Le '6' est toujours émis, juste par time::init() au lieu de init_tsc())

8 — Checklist de validation : timekeeping prêt pour bare-metal
Test	Condition de succès	Fail = erreur silencieuse détectée
ktime_get_ns() monotone 1M appels consécutifs	Chaque appel >= appel précédent (jamais de régression)	TIME-01 : seqlock insuffisant ou drift correction régressive
ktime_get_ns() depuis ISR (timer handler)	Pas de deadlock, retour en < 200ns	TIME-01 : Mutex dans le chemin ou seqlock qui boucle trop
Calibration TSC sur bare-metal (pas QEMU)	TSC Hz dans [2GHz, 6GHz] Écart CPUID nominal < 5%	TIME-02 : loop condition itérations Time-03 : IRQs perdues → Hz faux
Calibration sur VM avec Turbo Boost activé	TSC Hz stable à ±0.1% sur 100 mesures consécutives	TIME-04 : TSC non-invariant utilisé Turbo fausse la mesure
QEMU -smp 4 : ktime cohérent entre 4 CPUs	ktime_get_ns() sur CPU 0..3 dans les ±100ns l'un de l'autre	TIME-05 : TSC offset inter-CPU non mesuré → écarts détectés
Uptime 10 minutes : pas de wrap HPET 32-bit	ktime monotone sur 600 secondes sans saut	TIME-08 : wrapping_sub oublié Wrap → régression de 300s
Recalibration drift correction sous charge CPU 100%	tsc_hz stable ±500ppm pas de saut > 1ms	TIME-06 : preempt non désactivé Drift fausse le calcul
SWAPGS test SMP (bloquant)	current_thread_raw() sur CPU1 retourne le bon TCB	SCHED-01/02 : mauvais GS.base Current thread corrompu

📋  Ordre d'exécution des tests : d'abord 1 et 2 (monotonie + ISR-safe), puis 3 et 4 (calibration bare-metal), puis 5 (SMP coherence), puis 6 (uptime). Le test 8 (SWAPGS) est bloquant pour le SMP mais indépendant des tests 1-6.

