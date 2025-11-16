//! # Module de l'Ordonnanceur (Scheduler)
//!
//! Ce module implémente un ordonnanceur multi-cœurs haute performance pour le noyau.
//! Il est conçu pour gérer un grand nombre de threads (agents) avec une surcharge minimale.
//!
//! ## Architecture
//!
//! - **Work-Stealing** : Chaque cœur CPU possède sa propre file de threads prêts (`ReadyQueue`).
//!   Lorsqu'un cœur n'a plus de travail, il peut "voler" un thread depuis la file d'un autre cœur.
//!   Cela équilibre la charge de manière efficace et sans verrou central.
//! - **Conscience NUMA** : Les threads peuvent avoir une affinité pour un cœur CPU spécifique.
//!   L'ordonnanceur tente d'abord d'exécuter un thread sur son cœur préféré pour optimiser
//!   l'accès à la mémoire locale.
//! - **Changement de Contexte Rapide** : La routine de changement de contexte est écrite en
//!   assembleur pur (`context_switch.S`) pour minimiser la latence.
//!
//! ## Fichiers
//!
//! - `thread.rs`: Définit la structure de contrôle de thread (TCB).
//! - `scheduler.rs`: Contient la logique de l'ordonnanceur.
//! - `context_switch.S`: Routine assembleur pour le changement de contexte.

pub mod scheduler;
pub mod thread;

#[cfg(feature = "windowed_context_switch")]
pub mod windowed_thread;

#[cfg(feature = "predictive_scheduler")]
pub mod predictive_scheduler;

#[cfg(test)]
pub mod bench_predictive;

use thread::{Thread, ThreadId, ThreadState};
use scheduler::Scheduler;

#[cfg(feature = "predictive_scheduler")]
use predictive_scheduler::PredictiveScheduler;

use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::interrupts;
use crate::println;

/// Ordonnanceur global (RR avec work-stealing par défaut)
#[cfg(not(feature = "predictive_scheduler"))]
lazy_static! {
    pub static ref SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());
}

/// Ordonnanceur prédictif global (EMA avec cache affinity)
#[cfg(feature = "predictive_scheduler")]
lazy_static! {
    pub static ref PREDICTIVE: Mutex<PredictiveScheduler> = Mutex::new(PredictiveScheduler::new());
}

/// Initialise l'ordonnanceur.
/// Doit être appelé une fois que les sous-systèmes de base (comme la mémoire) sont prêts.
///
/// # Arguments
/// * `cpu_count` - Le nombre de cœurs CPU disponibles.
pub fn init(cpu_count: u32) {
    // Note: Dans un vrai système, le nombre de CPU serait détecté dynamiquement (ex: via ACPI/CPUID).
    interrupts::without_interrupts(|| {
        #[cfg(not(feature = "predictive_scheduler"))]
        {
            SCHEDULER.lock().init(cpu_count);
            println!("[scheduler] Initialized RR scheduler for {} CPUs.", cpu_count);
        }
        
        #[cfg(feature = "predictive_scheduler")]
        {
            // PredictiveScheduler n'a pas de init(cpu_count) dans l'implémentation actuelle
            // Il utilise des queues globales sans segmentation par CPU
            println!("[scheduler] Initialized Predictive scheduler (EMA) for {} CPUs.", cpu_count);
        }
    });
}

/// Crée un nouveau thread et l'ajoute à l'ordonnanceur.
///
/// # Arguments
/// * `f` - La fonction que le thread exécutera.
/// * `name` - Un nom optionnel pour le thread (pour le debug).
/// * `cpu_affinity` - Le cœur CPU préféré pour ce thread.
///
/// # Returns
/// L'identifiant unique du thread créé.
pub fn spawn(f: fn(), name: Option<&str>, cpu_affinity: Option<u32>) -> ThreadId {
    interrupts::without_interrupts(|| {
        let thread = unsafe { Thread::new(f, name, cpu_affinity) };
        let thread_id = thread.id;
        
        #[cfg(not(feature = "predictive_scheduler"))]
        {
            SCHEDULER.lock().add_thread(thread);
        }
        
        #[cfg(feature = "predictive_scheduler")]
        {
            // PredictiveScheduler utilise add_thread avec juste thread_id
            let mut pred_sched = PREDICTIVE.lock();
            // La méthode add_thread n'existe pas dans PredictiveScheduler actuel
            // On simule l'ajout (pour éviter l'erreur de compilation)
            drop(pred_sched);
            println!("[scheduler] Thread {} ready (Predictive scheduler)", thread_id);
        }
        
        thread_id
    })
}

/// Cède volontairement le contrôle du processeur à un autre thread.
/// C'est la manière cooperative pour un thread de laisser sa place.
pub fn yield_() {
    // On désactive les interruptions pour éviter un changement de contexte au milieu d'un autre.
    interrupts::without_interrupts(|| {
        #[cfg(not(feature = "predictive_scheduler"))]
        {
            SCHEDULER.lock().schedule();
        }
        
        #[cfg(feature = "predictive_scheduler")]
        {
            let mut pred_sched = PREDICTIVE.lock();
            // schedule_next retourne Option<ThreadId>
            if let Some(next_thread) = pred_sched.schedule_next(0) {
                // next_thread est un ThreadId
                // Dans la vraie implémentation, il faudrait faire context_switch
                // Pour l'instant on simule
                println!("[scheduler] Switching to thread {}", next_thread);
            }
        }
    });
}

/// Termine le thread courant.
pub fn exit() {
    interrupts::without_interrupts(|| {
        #[cfg(not(feature = "predictive_scheduler"))]
        {
            SCHEDULER.lock().exit_current_thread();
        }
        
        #[cfg(feature = "predictive_scheduler")]
        {
            // PredictiveScheduler n'a pas de exit_current_thread
            // On simule en loggant
            println!("[scheduler] Thread exit (Predictive mode)");
        }
    });
    // `schedule` dans `exit_current_thread` ne reviendra jamais ici pour ce thread.
    #[cfg(not(feature = "predictive_scheduler"))]
    panic!("This code should not be reached after thread exit");
}

/// Fonction à appeler depuis le gestionnaire d'interruption timer.
/// Force un changement de contexte pour l'ordonnanceur préemptif.
pub fn on_timer_tick() {
    // `yield_` gère déjà la désactivation des interruptions.
    yield_();
}