//! Séquence de boot optimisée avec parallélisation

use crate::{print, println};

/// Phases de boot (ordre d'exécution)
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum BootPhase {
    Critical,      // Doit bloquer (GDT, IDT, memory)
    Essential,     // Peut être parallèle (IPC, scheduler)
    Optional,      // Lazy init (drivers secondaires)
}

/// Tâche de boot
pub struct BootTask {
    pub name: &'static str,
    pub phase: BootPhase,
    pub init_fn: fn(),
}

/// Macro pour définir des tâches de boot
macro_rules! boot_task {
    ($name:expr, $phase:expr, $fn:expr) => {
        BootTask {
            name: $name,
            phase: $phase,
            init_fn: $fn,
        }
    };
}

/// Liste des tâches de boot (ordre optimisé)
pub static BOOT_TASKS: &[BootTask] = &[
    // Phase 1: CRITICAL - Architecture et Memory déjà initialisées dans kernel_main
    
    // Phase 2: ESSENTIAL (peut être parallèle si multi-core)
    boot_task!("Scheduler", BootPhase::Essential, || {
        crate::scheduler::init(4); // 4 CPUs par défaut
    }),
    boot_task!("IPC", BootPhase::Essential, || {
        crate::ipc::init();
    }),
    boot_task!("Syscalls", BootPhase::Essential, || {
        crate::syscall::init();
    }),

    // Phase 3: OPTIONAL (lazy init après boot)
    boot_task!("Drivers", BootPhase::Optional, || {
        crate::drivers::init();
    }),
];

/// Exécute la séquence de boot optimisée
pub fn run_boot_sequence() {
    println!("[BOOT] Démarrage séquence optimisée...");
    
    let start = crate::perf_counters::rdtsc();

    // Phase 1: Critical (bloquer)
    for task in BOOT_TASKS.iter().filter(|t| t.phase == BootPhase::Critical) {
        print!("  [BOOT] {} ... ", task.name);
        (task.init_fn)();
        println!("OK");
    }

    // Phase 2: Essential (pour l'instant séquentiel, TODO: parallèle)
    for task in BOOT_TASKS.iter().filter(|t| t.phase == BootPhase::Essential) {
        print!("  [BOOT] {} ... ", task.name);
        (task.init_fn)();
        println!("OK");
    }

    let end = crate::perf_counters::rdtsc();
    let cycles = end - start;
    let time_ms = cycles / 3_000_000; // Assume 3 GHz CPU

    println!("[BOOT] Core boot complété en {} ms ({} cycles)", time_ms, cycles);

    // Phase 3: Optional (lazy init en arrière-plan)
    println!("[BOOT] Phase 3: Optional modules (deferred)...");
    lazy_init_drivers();
}

/// Init lazy pour drivers non-critiques
pub fn lazy_init_drivers() {
    for task in BOOT_TASKS.iter().filter(|t| t.phase == BootPhase::Optional) {
        print!("  [BOOT] {} ... ", task.name);
        (task.init_fn)();
        println!("OK");
    }
}
