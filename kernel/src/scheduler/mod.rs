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

pub mod thread;
pub mod scheduler;

use thread::{Thread, ThreadId, ThreadState};
use scheduler::Scheduler;
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::interrupts;

/// Ordonnanceur global, initialisé au démarrage.
lazy_static! {
    pub static ref SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());
}

/// Initialise l'ordonnanceur.
/// Doit être appelé une fois que les sous-systèmes de base (comme la mémoire) sont prêts.
///
/// # Arguments
/// * `cpu_count` - Le nombre de cœurs CPU disponibles.
pub fn init(cpu_count: u32) {
    // Note: Dans un vrai système, le nombre de CPU serait détecté dynamiquement (ex: via ACPI/CPUID).
    interrupts::without_interrupts(|| {
        SCHEDULER.lock().init(cpu_count);
    });
    println!("[scheduler] Initialized for {} CPUs.", cpu_count);
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
        let thread = Thread::new(f, name, cpu_affinity);
        let thread_id = thread.id;
        SCHEDULER.lock().add_thread(thread);
        thread_id
    })
}

/// Cède volontairement le contrôle du processeur à un autre thread.
/// C'est la manière cooperative pour un thread de laisser sa place.
pub fn yield_() {
    // On désactive les interruptions pour éviter un changement de contexte au milieu d'un autre.
    interrupts::without_interrupts(|| {
        SCHEDULER.lock().schedule();
    });
}

/// Termine le thread courant.
pub fn exit() {
    interrupts::without_interrupts(|| {
        SCHEDULER.lock().exit_current_thread();
    });
    // `schedule` dans `exit_current_thread` ne reviendra jamais ici pour ce thread.
    panic!("This code should not be reached after thread exit");
}

/// Fonction à appeler depuis le gestionnaire d'interruption timer.
/// Force un changement de contexte pour l'ordonnanceur préemptif.
pub fn on_timer_tick() {
    // `yield_` gère déjà la désactivation des interruptions.
    yield_();
}