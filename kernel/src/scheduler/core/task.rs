// kernel/src/scheduler/core/task.rs
//
// Fichier : kernel/src/scheduler/core/task.rs
// Rôle    : ThreadControlBlock — Layout canonique 256B — GI-01 Étape 9.
//
// DÉPENDANCES :
//   - Aucune dépendance externe (types auto-contenus)
//   - switch_asm.s utilise les offsets [8] kstack_ptr et [56] cr3_phys
//
// INVARIANTS :
//   - TCB = 256 bytes EXACT (#[repr(C, align(64))]) — vérifié compile-time
//   - kstack_ptr  à l'offset [8]   — HARDCODÉ dans switch_asm.s
//   - cr3_phys    à l'offset [56]  — HARDCODÉ dans switch_asm.s (KPTI)
//   - fpu_state_ptr à l'offset [232] — lu par ExoPhoenix/Kernel B
//   - rq_next     à l'offset [240] — intrusive runqueue
//   - rq_prev     à l'offset [248] — intrusive runqueue
//
// SCHED_STATE (AtomicU64 à l'offset [24]) — encodage compact :
//   bits [7:0]  = TaskState value (u8)
//   bit  [8]    = signal_pending flag
//   bit  [9]    = KTHREAD flag
//   bit  [10]   = FPU_LOADED flag
//   bit  [11]   = NEED_RESCHED flag
//   bits [31:12]= flags scheduler étendus (réservés)
//   bits [63:32]= pid (ProcessId.0 as u32)
//
// SÉCURITÉ ISR :
//   - sched_state : AtomicU64 — accès atomiques [ISR-SAFE]
//   - fpu_state_ptr, kstack_ptr, cr3_phys : u64 [THREAD-ONLY pour write]
//   - rq_next/rq_prev : écrits sous spinlock RunQueue [THREAD-ONLY]
//
// SOURCE DE VÉRITÉ :
//   ExoOS_Architecture_v7.md §3.2, ExoOS_Kernel_Types_v10.md,
//   ExoOS_Corrections_01 CORR-01, GI-01_Types_TCB_SSR.md §7,
//   ExoPhoenix_Spec_v6.md §3 (offsets alignés sur v7 — CORR-01)

use core::sync::atomic::{AtomicU64, Ordering};
// NOTE : les imports AtomicBool/AtomicU32/AtomicU8 supprimés — remplacés par
// l'encodage dans sched_state (AtomicU64). Voir constantes SCHED_*_BIT.

// ─── Types auxiliaires ───────────────────────────────────────────────────────

/// Identifiant de thread — unique dans tout le système (64 bits).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(transparent)]
pub struct ThreadId(pub u64);

/// Identifiant de processus (32 bits, ABI POSIX compatible).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(transparent)]
pub struct ProcessId(pub u32);

/// Identifiant CPU logique.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(transparent)]
pub struct CpuId(pub u32);

/// Priorité d'ordonnancement (0 = RT_MAX, 139 = NORMAL_MIN, 140 = IDLE).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(transparent)]
pub struct Priority(pub u8);

impl Priority {
    pub const RT_MAX:         Self = Self(0);
    pub const RT_MIN:         Self = Self(99);
    pub const NORMAL_MAX:     Self = Self(100);
    pub const NORMAL_DEFAULT: Self = Self(120);
    pub const NORMAL_MIN:     Self = Self(139);
    pub const IDLE:           Self = Self(140);

    /// `nice` Linux-compatible (-20..+19) → priority 100..139.
    #[inline(always)]
    pub fn from_nice(nice: i8) -> Self {
        Self((120i16 + nice.clamp(-20, 19) as i16) as u8)
    }

    #[inline(always)]
    pub fn is_realtime(self) -> bool { self.0 <= 99 }

    /// Poids CFS Linux-compatible (prio_to_weight[40]).
    #[inline(always)]
    pub fn cfs_weight(self) -> u32 {
        const W: [u32; 40] = [
            88761, 71755, 56483, 46273, 36291,
            29154, 23254, 18705, 14949, 11916,
             9548,  7620,  6100,  4904,  3906,
             3121,  2501,  1991,  1586,  1277,
             1024,   820,   655,   526,   423,
              335,   272,   215,   172,   137,
              110,    87,    70,    56,    45,
               36,    29,    23,    18,    15,
        ];
        if self.0 < 100 { return 1024; }
        W[(self.0 as i32 - 120 + 20).clamp(0, 39) as usize]
    }
}

/// Politique d'ordonnancement.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum SchedPolicy {
    #[default]
    Normal     = 0,
    Batch      = 1,
    Fifo       = 2,
    RoundRobin = 3,
    Deadline   = 4,
    Idle       = 5,
}

/// État du thread dans la machine d'états.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum TaskState {
    Runnable        = 0,
    Running         = 1,
    Sleeping        = 2,
    Uninterruptible = 3,
    Stopped         = 4,
    Zombie          = 5,
    Dead            = 6,
}

impl TaskState {
    /// Convertit depuis u8 — `Dead` pour toute valeur inconnue.
    #[inline(always)]
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Runnable,
            1 => Self::Running,
            2 => Self::Sleeping,
            3 => Self::Uninterruptible,
            4 => Self::Stopped,
            5 => Self::Zombie,
            _ => Self::Dead,
        }
    }
}

/// Paramètres EDF (conservé pour compat ; non embarqué dans le TCB canonique).
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct DeadlineParams {
    pub runtime_ns:  u64,
    pub deadline_ns: u64,
    pub period_ns:   u64,
}

/// Contexte CPU (conservé pour compat ; non embarqué dans le TCB canonique).
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct CpuContext {
    pub gpr:     [u64; 8],
    pub rip:     u64,
    pub rsp_usr: u64,
    pub rflags:  u64,
    pub cs_ss:   u64,
    pub cr2:     u64,
}

impl Default for CpuContext {
    #[inline]
    fn default() -> Self {
        Self { gpr: [0u64; 8], rip: 0, rsp_usr: 0, rflags: 0, cs_ss: 0, cr2: 0 }
    }
}

// ─── Constantes sched_state ──────────────────────────────────────────────────
//
// sched_state (AtomicU64, offset [24]) — encodage compact :
//   bits  [7:0]  = TaskState value
//   bit   [8]    = signal_pending
//   bit   [9]    = KTHREAD
//   bit   [10]   = FPU_LOADED
//   bit   [11]   = NEED_RESCHED
//   bit   [12]   = EXITING
//   bits [31:13] = réservés
//   NOTE : pid n'est PAS encodé dans sched_state — champ direct `pid: ProcessId` à [92]

const SCHED_STATE_MASK:      u64 = 0xFF;
/// Signal en attente (bit 8).
pub const SCHED_SIGNAL_BIT:       u64 = 1 << 8;
/// Thread kernel, jamais userspace (bit 9).
pub const SCHED_KTHREAD_BIT:      u64 = 1 << 9;
/// FPU chargée dans les registres physiques (bit 10).
pub const SCHED_FPU_LOADED_BIT:   u64 = 1 << 10;
/// Préemption demandée — TIF_NEED_RESCHED (bit 11).
pub const SCHED_NEED_RESCHED_BIT: u64 = 1 << 11;
/// Thread en cours de terminaison (bit 12).
pub const SCHED_EXITING_BIT:      u64 = 1 << 12;
/// Thread idle (bit 13).
pub const SCHED_IDLE_BIT:         u64 = 1 << 13;
/// Thread en reclaim mémoire — allocation FPU interdite (bit 14).
pub const SCHED_IN_RECLAIM_BIT:   u64 = 1 << 14;

/// Flags compat pour l'ancien code utilisant `task_flags::*`.
/// Ces constantes ne sont PAS utilisées par le TCB canonique (encodage sched_state).
pub mod task_flags {
    pub const KTHREAD:          u32 = 1 << 0;
    pub const FPU_LOADED:       u32 = 1 << 1;
    pub const EXITING:          u32 = 1 << 2;
    pub const WAKEUP_SPURIOUS:  u32 = 1 << 3;
    pub const NEED_RESCHED:     u32 = 1 << 4;
    pub const IN_RECLAIM:       u32 = 1 << 5;
    pub const MIGRATED:         u32 = 1 << 6;
    pub const PTRACE:           u32 = 1 << 7;
    pub const IN_WAIT_QUEUE:    u32 = 1 << 8;
    pub const IS_IDLE:          u32 = 1 << 9;
}

// ─── ThreadControlBlock ──────────────────────────────────────────────────────
//
// Layout exact 256 bytes, align(64).
//
// Cache-line 1 [0..64]   — hot path pick_next_task()
//   [0]  tid:          u64        identifiant thread
//   [8]  kstack_ptr:   u64        RSP kernel  ← switch_asm.s OFFSET HARDCODÉ
//   [16] priority:     u8
//   [17] policy:       u8
//   [18] _pad0:        [u8; 6]
//   [24] sched_state:  AtomicU64  pid|flags|signal|state
//   [32] vruntime:     AtomicU64  vruntime CFS (ns)
//   [40] deadline_abs: u64        deadline EDF absolue (ns depuis boot)
//   [48] cpu_affinity: AtomicU64  bitmask affinité CPU
//   [56] cr3_phys:     u64        CR3 espace adressage  ← switch_asm.s OFFSET HARDCODÉ
//
// Cache-line 2 [64..128]  — warm (context switch)
//   [64]  cpu_id:      AtomicU64  CPU courant
//   [72]  fs_base:     u64        FS base (TLS)
//   [80]  user_gs_base:u64        GS base userspace
//   [88]  pkrs:        u32        PKRS
//   [92]  _pad1:       [u8; 4]
//   [96]  signal_mask: AtomicU64  bitmask signaux bloqués
//   [104] dl_runtime:  u64        budget EDF (ns/période)
//   [112] dl_period:   u64        période EDF (ns)
//   [120] _pad2:       [u8; 8]
//
// Cache-lines 3-4 [128..256] — cold
//   [128] run_time_acc:  u64
//   [136] switch_count:  u64
//   [144] _cold_reserve: [u8; 88]  (144+88=232)
//     ExoShield v1.0 extensions within _cold_reserve :
//       [144] shadow_stack_token : u64   (PKS domain TcbHot)
//       [152] cet_flags          : u8    (bit 0=CET_EN, bit 1=IBT, bit 2=TOKEN_VALID)
//       [153] threat_score_u8    : u8    (0..=100)
//       [160] pt_buffer_phys     : u64   (Phase 4, LBR/PT futur)
//       [168] affinity_hi[0]     : u64   (CPUs 64..127)
//       [176] affinity_hi[1]     : u64   (CPUs 128..191)
//       [184] affinity_hi[2]     : u64   (CPUs 192..255)
//       [192..232] réservé
//   [232] fpu_state_ptr: u64       ← ExoPhoenix OFFSET HARDCODÉ
//   [240] rq_next:       u64       intrusive RunQueue
//   [248] rq_prev:       u64       intrusive RunQueue

#[repr(C, align(64))]
pub struct ThreadControlBlock {
    // ═══ Cache-line 1 [0..64] ═══════════════════════════════════════════════
    pub tid:          u64,         // [0]
    pub kstack_ptr:   u64,         // [8]   switch_asm.s HARDCODED
    pub priority:     Priority,    // [16]
    pub policy:       SchedPolicy,  // [17]
    _pad0:            [u8; 6],     // [18]
    pub sched_state:  AtomicU64,   // [24]
    pub vruntime:     AtomicU64,   // [32]
    pub deadline_abs: AtomicU64,   // [40]  deadline EDF absolue (ns depuis boot)
    pub cpu_affinity: AtomicU64,   // [48]
    pub cr3_phys:     u64,         // [56]  switch_asm.s HARDCODED
    // ═══ Cache-line 2 [64..128] ══════════════════════════════════════════════
    pub cpu_id:       AtomicU64,   // [64]
    pub fs_base:      u64,         // [72]
    pub user_gs_base: u64,         // [80]
    pub pkrs:         u32,         // [88]
    pub pid:          ProcessId,   // [92]  champ direct (compat)
    pub signal_mask:  AtomicU64,   // [96]
    pub dl_runtime:   u64,         // [104]
    pub dl_period:    u64,         // [112]
    _pad2:            [u8; 8],     // [120]
    // ═══ Cache-lines 3-4 [128..256] ══════════════════════════════════════════
    pub run_time_acc:  u64,        // [128]
    pub switch_count:  u64,        // [136]
    pub(crate) _cold_reserve: [u8; 88], // [144]  (144+88=232)
    pub fpu_state_ptr: u64,        // [232]  ExoPhoenix HARDCODED
    pub rq_next:       u64,        // [240]  intrusive runqueue
    pub rq_prev:       u64,        // [248]  intrusive runqueue
}                                  // total = 256B ✓

// ─── Assertions layout statiques ─────────────────────────────────────────────
use core::mem::{size_of, offset_of};

const _: () = assert!(size_of::<ThreadControlBlock>() == 256,
    "TCB: taille doit être exactement 256 bytes");
const _: () = assert!(core::mem::align_of::<ThreadControlBlock>() == 64,
    "TCB: alignement doit être 64 bytes");
const _: () = assert!(offset_of!(ThreadControlBlock, kstack_ptr) == 8,
    "TCB: kstack_ptr doit être à l'offset 8 (switch_asm.s)");
const _: () = assert!(offset_of!(ThreadControlBlock, sched_state) == 24,
    "TCB: sched_state doit être à l'offset 24");
const _: () = assert!(offset_of!(ThreadControlBlock, cr3_phys) == 56,
    "TCB: cr3_phys doit être à l'offset 56 (switch_asm.s)");
const _: () = assert!(offset_of!(ThreadControlBlock, fpu_state_ptr) == 232,
    "TCB: fpu_state_ptr doit être à l'offset 232 (ExoPhoenix)");
const _: () = assert!(offset_of!(ThreadControlBlock, rq_next) == 240,
    "TCB: rq_next doit être à l'offset 240");
const _: () = assert!(offset_of!(ThreadControlBlock, rq_prev) == 248,
    "TCB: rq_prev doit être à l'offset 248");

// ─── Assertions layout ExoShield v1.0 — extensions _cold_reserve ──────────
//
// Les champs ExoShield dans _cold_reserve utilisent des offsets relatifs au
// début du champ _cold_reserve (offset TCB 144). Les accès se font via
// les helpers unsafe tcb_write_cold_u64 / tcb_read_cold_u64 dans exocage.rs.
//
// Ces assertions garantissent que :
// 1. _cold_reserve commence bien à l'offset 144
// 2. Le sous-champ shadow_stack_token (_cold_reserve[0..7]) = TCB offset 144
// 3. Le sous-champ cet_flags          (_cold_reserve[8])    = TCB offset 152
// 4. Le sous-champ threat_score_u8    (_cold_reserve[9])    = TCB offset 153
// 5. Le sous-champ pt_buffer_phys     (_cold_reserve[16..23])= TCB offset 160
// 6. Les offsets hardcodés du TCB restent inchangés

const _: () = assert!(offset_of!(ThreadControlBlock, _cold_reserve) == 144,
    "TCB: _cold_reserve doit commencer à l'offset 144 (ExoShield shadow_stack_token)");
const _: () = assert!(offset_of!(ThreadControlBlock, _cold_reserve) + 0  == 144,
    "TCB ExoShield: shadow_stack_token doit être à l'offset absolu 144");
const _: () = assert!(offset_of!(ThreadControlBlock, _cold_reserve) + 8  == 152,
    "TCB ExoShield: cet_flags doit être à l'offset absolu 152");
const _: () = assert!(offset_of!(ThreadControlBlock, _cold_reserve) + 9  == 153,
    "TCB ExoShield: threat_score_u8 doit être à l'offset absolu 153");
const _: () = assert!(offset_of!(ThreadControlBlock, _cold_reserve) + 16 == 160,
    "TCB ExoShield: pt_buffer_phys doit être à l'offset absolu 160");
const _: () = assert!(offset_of!(ThreadControlBlock, _cold_reserve) + 24 == 168,
    "TCB scheduler: affinity_hi[0] doit être à l'offset absolu 168");
const _: () = assert!(offset_of!(ThreadControlBlock, _cold_reserve) + 32 == 176,
    "TCB scheduler: affinity_hi[1] doit être à l'offset absolu 176");
const _: () = assert!(offset_of!(ThreadControlBlock, _cold_reserve) + 40 == 184,
    "TCB scheduler: affinity_hi[2] doit être à l'offset absolu 184");
const _: () = assert!(offset_of!(ThreadControlBlock, _cold_reserve) + 88 == 232,
    "TCB ExoShield: _cold_reserve se termine à l'offset 232 (fpu_state_ptr)");
const _: () = assert!(size_of::<ThreadControlBlock>() == 256,
    "TCB: taille doit rester 256 bytes (ExoShield extensions ne dépassent pas)");

// FIX-CET-01: pl0_ssp utilise _cold_reserve[48..56] = offset absolu 192..200 < 232.
const _: () = assert!(
    offset_of!(ThreadControlBlock, _cold_reserve) + 48 + 8 <= 232,
    "FIX-CET-01: pl0_ssp doit tenir dans _cold_reserve avant fpu_state_ptr (232)"
);

// ─── impl ThreadControlBlock ─────────────────────────────────────────────────

impl ThreadControlBlock {
    /// Crée un nouveau TCB utilisateur.
    pub fn new(
        tid:              ThreadId,
        pid:              ProcessId,
        policy:           SchedPolicy,
        prio:             Priority,
        cr3_phys:         u64,
        kernel_stack_top: u64,
    ) -> Self {
        let mut cold_reserve = [0u8; 88];
        cold_reserve[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
        cold_reserve[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
        cold_reserve[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
        Self {
            tid:           tid.0,
            kstack_ptr:    kernel_stack_top,
            priority:      prio,
            policy:        policy,
            _pad0:         [0u8; 6],
            sched_state:   AtomicU64::new(TaskState::Runnable as u64),
            vruntime:      AtomicU64::new(0),
            deadline_abs:  AtomicU64::new(0),
            cpu_affinity:  AtomicU64::new(!0u64),
            cr3_phys,
            cpu_id:        AtomicU64::new(0),
            fs_base:       0,
            user_gs_base:  0,
            pkrs:          0,
            pid,
            signal_mask:   AtomicU64::new(0),
            dl_runtime:    0,
            dl_period:     0,
            _pad2:         [0u8; 8],
            run_time_acc:  0,
            switch_count:  0,
            _cold_reserve: cold_reserve,
            fpu_state_ptr: 0,
            rq_next:       0,
            rq_prev:       0,
        }
    }

    /// Thread kernel (pid=0, flag KTHREAD dans sched_state).
    pub fn new_kthread(tid: ThreadId, cr3_phys: u64, kernel_stack_top: u64) -> Self {
        let tcb = Self::new(
            tid, ProcessId(0), SchedPolicy::Normal,
            Priority::NORMAL_DEFAULT, cr3_phys, kernel_stack_top,
        );
        tcb.sched_state.fetch_or(SCHED_KTHREAD_BIT, Ordering::Relaxed);
        tcb
    }

    // ─── Accesseurs sched_state ───────────────────────────────────────────────

    /// Lit l'état courant du thread.
    #[inline(always)]
    pub fn task_state(&self) -> TaskState {
        TaskState::from_u8(
            (self.sched_state.load(Ordering::Acquire) & SCHED_STATE_MASK) as u8
        )
    }

    /// Alias backward-compat.
    #[inline(always)]
    pub fn state(&self) -> TaskState { self.task_state() }

    /// Définit l'état du thread (Release).
    #[inline(always)]
    pub fn set_task_state(&self, s: TaskState) {
        loop {
            let cur = self.sched_state.load(Ordering::Relaxed);
            let new = (cur & !SCHED_STATE_MASK) | (s as u64);
            if self.sched_state.compare_exchange_weak(
                cur, new, Ordering::Release, Ordering::Relaxed,
            ).is_ok() { break; }
        }
    }

    /// Alias backward-compat.
    #[inline(always)]
    pub fn set_state(&self, s: TaskState) { self.set_task_state(s); }

    /// Transition CAS — retourne `true` si réussie.
    #[inline(always)]
    pub fn try_transition(&self, from: TaskState, to: TaskState) -> bool {
        loop {
            let cur = self.sched_state.load(Ordering::Relaxed);
            if (cur & SCHED_STATE_MASK) as u8 != from as u8 { return false; }
            let next = (cur & !SCHED_STATE_MASK) | (to as u64);
            match self.sched_state.compare_exchange_weak(
                cur, next, Ordering::AcqRel, Ordering::Relaxed,
            ) {
                Ok(_)  => return true,
                Err(_) => continue,
            }
        }
    }

    /// Lit le PID du thread (champ direct).
    #[inline(always)]
    pub fn pid_val(&self) -> ProcessId { self.pid }

    /// Alias backward-compat pour les appelants qui utilisait la syntaxe méthode.
    #[inline(always)]
    pub fn get_pid(&self) -> ProcessId { self.pid }

    /// Vrai si un signal est en attente.
    #[inline(always)]
    pub fn has_signal_pending(&self) -> bool {
        self.sched_state.load(Ordering::Relaxed) & SCHED_SIGNAL_BIT != 0
    }

    /// Alias backward-compat.
    #[inline(always)]
    pub fn signal_pending(&self) -> bool { self.has_signal_pending() }

    /// Positionne signal_pending (appelé UNIQUEMENT depuis process/signal/).
    #[inline(always)]
    pub fn set_signal_pending(&self) {
        self.sched_state.fetch_or(SCHED_SIGNAL_BIT, Ordering::Release);
    }

    /// Efface signal_pending après livraison.
    #[inline(always)]
    pub fn clear_signal_pending(&self) {
        self.sched_state.fetch_and(!SCHED_SIGNAL_BIT, Ordering::Release);
    }

    // ─── Flags ────────────────────────────────────────────────────────────────

    #[inline(always)] pub fn is_kthread(&self) -> bool {
        self.sched_state.load(Ordering::Relaxed) & SCHED_KTHREAD_BIT != 0
    }
    #[inline(always)] pub fn fpu_loaded(&self) -> bool {
        self.sched_state.load(Ordering::Relaxed) & SCHED_FPU_LOADED_BIT != 0
    }
    #[inline(always)] pub fn need_resched(&self) -> bool {
        self.sched_state.load(Ordering::Relaxed) & SCHED_NEED_RESCHED_BIT != 0
    }
    #[inline(always)] pub fn is_exiting(&self) -> bool {
        self.sched_state.load(Ordering::Relaxed) & SCHED_EXITING_BIT != 0
    }
    #[inline(always)] pub fn is_idle(&self) -> bool {
        self.sched_state.load(Ordering::Relaxed) & SCHED_IDLE_BIT != 0
    }

    #[inline(always)]
    pub fn set_fpu_loaded(&self, loaded: bool) {
        if loaded {
            self.sched_state.fetch_or(SCHED_FPU_LOADED_BIT, Ordering::Relaxed);
        } else {
            self.sched_state.fetch_and(!SCHED_FPU_LOADED_BIT, Ordering::Relaxed);
        }
    }

    /// Demande une préemption (thread-safe depuis tout CPU).
    #[inline(always)]
    pub fn request_preemption(&self) {
        self.sched_state.fetch_or(SCHED_NEED_RESCHED_BIT, Ordering::Release);
    }

    // ── FIX-CET-01 : CET Shadow Stack Pointer per-thread ─────────────────────
    //
    // Utilise _cold_reserve[48..56] (offset TCB absolu = 144 + 48 = 192).
    // Non-overlapping avec les champs ExoShield documentés (0..40).
    // Valeur initiale = 0 (CET non activé pour ce thread).

    /// Lit MSR_IA32_PL0_SSP sauvegardé dans ce TCB.
    #[inline(always)]
    pub fn pl0_ssp(&self) -> u64 {
        // SAFETY: _cold_reserve est [u8; 88], offset 48..56 < 88.
        unsafe {
            core::ptr::read_unaligned(
                self._cold_reserve.as_ptr().add(48) as *const u64
            )
        }
    }

    /// Sauvegarde MSR_IA32_PL0_SSP dans ce TCB (_cold_reserve[48..56]).
    #[inline(always)]
    pub fn set_pl0_ssp(&mut self, ssp: u64) {
        // SAFETY: offset 48..56 dans _cold_reserve[88], non-overlapping ExoShield (0..40).
        unsafe {
            core::ptr::write_unaligned(
                self._cold_reserve.as_mut_ptr().add(48) as *mut u64,
                ssp,
            )
        }
    }

    /// Lit et efface NEED_RESCHED atomiquement.
    #[inline(always)]
    pub fn take_need_resched(&self) -> bool {
        self.sched_state.fetch_and(!SCHED_NEED_RESCHED_BIT, Ordering::AcqRel)
            & SCHED_NEED_RESCHED_BIT != 0
    }

    // ─── Scheduling ───────────────────────────────────────────────────────────

    #[inline(always)]
    pub fn current_cpu(&self) -> CpuId {
        CpuId(self.cpu_id.load(Ordering::Acquire) as u32)
    }

    #[inline(always)]
    pub fn assign_cpu(&self, cpu: CpuId) {
        self.cpu_id.store(cpu.0 as u64, Ordering::Release);
    }

    /// Vrai si le thread peut tourner sur le CPU donné.
    #[inline(always)]
    pub fn allowed_on(&self, cpu: CpuId) -> bool {
        self.cpu_affinity_mask().contains(cpu)
    }

    #[inline(always)]
    fn affinity_ext_word(&self, word_index: usize) -> &AtomicU64 {
        let offset = match word_index {
            1 => 24,
            2 => 32,
            3 => 40,
            _ => panic!("affinity_ext_word: word_index hors plage"),
        };
        // SAFETY: ces offsets 8-alignés de `_cold_reserve` sont réservés aux
        // trois mots d'affinité CPU supplémentaires du scheduler.
        unsafe { &*(self._cold_reserve.as_ptr().add(offset) as *const AtomicU64) }
    }

    #[inline(always)]
    pub fn cpu_affinity_mask(&self) -> crate::scheduler::smp::affinity::CpuSet {
        crate::scheduler::smp::affinity::CpuSet::new([
            self.cpu_affinity.load(Ordering::Acquire),
            self.affinity_ext_word(1).load(Ordering::Acquire),
            self.affinity_ext_word(2).load(Ordering::Acquire),
            self.affinity_ext_word(3).load(Ordering::Acquire),
        ])
    }

    #[inline(always)]
    pub fn set_cpu_affinity_mask(&self, mask: crate::scheduler::smp::affinity::CpuSet) {
        self.cpu_affinity.store(mask.bits[0], Ordering::Release);
        self.affinity_ext_word(1).store(mask.bits[1], Ordering::Release);
        self.affinity_ext_word(2).store(mask.bits[2], Ordering::Release);
        self.affinity_ext_word(3).store(mask.bits[3], Ordering::Release);
    }

    #[inline(always)]
    pub fn set_cpu_affinity_single(&self, cpu: CpuId) {
        self.set_cpu_affinity_mask(crate::scheduler::smp::affinity::CpuSet::single(cpu));
    }

    /// Avance le vruntime CFS (delta_ns, weight = priority.cfs_weight()).
    #[inline(always)]
    pub fn advance_vruntime(&self, delta_ns: u64, weight: u32) {
        const NICE_0_LOAD: u64 = 1024;
        let weighted = if weight == 0 { delta_ns }
            else { delta_ns.saturating_mul(NICE_0_LOAD) / weight as u64 };
        self.vruntime.fetch_add(weighted, Ordering::Release);
    }
}

// SAFETY: champs mutables gérés par atomiques ; kstack_ptr/cr3_phys/fpu_state_ptr
// modifiés uniquement par le CPU propriétaire (invariants scheduler).
unsafe impl Send for ThreadControlBlock {}
unsafe impl Sync for ThreadControlBlock {}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques par thread — séparées du TCB.
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs de performance par thread — alloués séparément du TCB (TCB = 256B).
#[repr(C, align(64))]
pub struct TaskStats {
    /// Context switches volontaires (sleep, yield, mutex wait…).
    pub voluntary_switches:   AtomicU64,
    /// Context switches involontaires (préemptions timer + IPI).
    pub involuntary_switches: AtomicU64,
    /// Temps total en Running (ns).
    pub run_time_ns:          AtomicU64,
    /// Temps total bloqué (sleep + uninterruptible, ns).
    pub blocked_time_ns:      AtomicU64,
    /// Migrations inter-CPU.
    pub migrations:           AtomicU64,
    /// Page faults utilisateur.
    pub page_faults:          AtomicU64,
    /// Timestamp dernier démarrage sur CPU (ns monotone).
    pub last_start_ns:        AtomicU64,
    /// Priority inversions détectées par le kernel mutex.
    pub priority_inversions:  AtomicU64,
}

impl TaskStats {
    pub const fn new() -> Self {
        Self {
            voluntary_switches:   AtomicU64::new(0),
            involuntary_switches: AtomicU64::new(0),
            run_time_ns:          AtomicU64::new(0),
            blocked_time_ns:      AtomicU64::new(0),
            migrations:           AtomicU64::new(0),
            page_faults:          AtomicU64::new(0),
            last_start_ns:        AtomicU64::new(0),
            priority_inversions:  AtomicU64::new(0),
        }
    }

    #[inline(always)]
    pub fn record_run_start(&self, now_ns: u64) {
        self.last_start_ns.store(now_ns, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn record_run_end(&self, now_ns: u64) {
        let start = self.last_start_ns.load(Ordering::Relaxed);
        if now_ns > start {
            self.run_time_ns.fetch_add(now_ns - start, Ordering::Relaxed);
        }
    }

    #[inline(always)]
    pub fn record_switch(&self, voluntary: bool) {
        if voluntary { self.voluntary_switches.fetch_add(1, Ordering::Relaxed); }
        else { self.involuntary_switches.fetch_add(1, Ordering::Relaxed); }
    }
}
