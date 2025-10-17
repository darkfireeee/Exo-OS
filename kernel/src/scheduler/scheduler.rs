//! # Logique de l'Ordonnanceur
//!
//! Ce fichier contient le cœur de l'ordonnanceur. Il gère les files de threads
//! par CPU, implémente la logique de work-stealing et orchestre les changements
//! de contexte.

use crate::scheduler::thread::{Thread, ThreadId, ThreadState, ThreadContext};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::vec;
use crate::println;
use alloc::sync::Arc;
use crossbeam_queue::SegQueue;
use spin::Mutex;

/// Structure représentant l'ordonnanceur.
pub struct Scheduler {
    /// Liste des files de threads prêts, une par CPU.
    /// `SegQueue` est une file lock-free, parfaite pour le work-stealing.
    ready_queues: Vec<SegQueue<ThreadId>>,
    /// Table de tous les threads actifs, indexés par leur ID.
    /// `Arc<Mutex<>>` permet un accès partagé et sécurisé entre les cœurs.
    threads: BTreeMap<ThreadId, Arc<Mutex<Thread>>>,
    /// Le thread actuellement en cours d'exécution sur chaque CPU.
    current_threads: Vec<Option<ThreadId>>,
    /// Nombre de cœurs CPU gérés.
    cpu_count: u32,
}

impl Scheduler {
    /// Crée une nouvelle instance de l'ordonnanceur.
    pub const fn new() -> Self {
        Self {
            ready_queues: Vec::new(),
            threads: BTreeMap::new(),
            current_threads: Vec::new(),
            cpu_count: 0,
        }
    }

    /// Initialise l'ordonnanceur avec un nombre donné de cœurs CPU.
    pub fn init(&mut self, cpu_count: u32) {
        self.cpu_count = cpu_count;
        self.ready_queues = (0..cpu_count).map(|_| SegQueue::new()).collect();
        self.current_threads = vec![None; cpu_count as usize];
    }

    /// Ajoute un nouveau thread au système.
    pub fn add_thread(&mut self, thread: Thread) {
        let thread_id = thread.id;
        let thread_arc = Arc::new(Mutex::new(thread));
        
        // Déterminer sur quelle file mettre le thread en fonction de son affinité.
        let target_cpu = thread_arc
            .lock()
            .cpu_affinity
            .unwrap_or_else(|| {
                // Si pas d'affinité, on utilise une politique round-robin simple.
                (thread_id as u32) % self.cpu_count
            });

        self.threads.insert(thread_id, thread_arc);
        self.ready_queues[target_cpu as usize].push(thread_id);
    }

    /// Orchestre un changement de contexte.
    /// C'est la fonction principale appelée par `yield_`, `exit`, ou les interruptions.
    pub fn schedule(&mut self) {
        // Obtenir l'ID du CPU actuel. Dans un vrai noyau, cela viendrait de registres spécifiques (ex: GS base).
        // Pour cet exemple, nous simulons être sur le CPU 0.
        let current_cpu_id = 0; 

        // 1. Récupérer le thread actuel.
        let old_thread_id = self.current_threads[current_cpu_id].take();

        // 2. Si un thread était en cours d'exécution, le remettre dans une file `Ready`.
        if let Some(id) = old_thread_id {
            if let Some(thread) = self.threads.get(&id) {
                let mut thread = thread.lock();
                if thread.state == ThreadState::Running {
                    thread.state = ThreadState::Ready;
                }
                // On ne remet pas le thread dans la file s'il est `Blocked` ou `Exited`.
                if thread.state == ThreadState::Ready {
                    // On le remet dans sa file préférée.
                    let target_cpu = thread.cpu_affinity.unwrap_or(current_cpu_id as u32);
                    self.ready_queues[target_cpu as usize].push(id);
                }
            }
        }

        // 3. Trouver le prochain thread à exécuter (logique de work-stealing).
        let next_thread_id = self.find_next_thread(current_cpu_id as u32);

        // 4. Mettre à jour le thread courant et effectuer le changement de contexte.
        if let Some(id) = next_thread_id {
            self.current_threads[current_cpu_id] = Some(id);
            if let Some(thread) = self.threads.get(&id) {
                thread.lock().state = ThreadState::Running;
            }
            
            // Récupérer les contextes.
            let old_context_ptr = old_thread_id
                .and_then(|id| self.threads.get(&id))
                .map(|t| &mut t.lock().context as *mut _)
                .unwrap_or(core::ptr::null_mut());
            let new_context_ptr = self.threads.get(&id).unwrap().lock().context.rsp().as_u64();

            // Effectuer le changement de contexte en assembleur.
            // C'est un point de non-retour pour l'ancien thread.
            unsafe {
                context_switch(old_context_ptr, new_context_ptr);
            }
        } else {
            // Aucun thread à exécuter. On peut mettre le CPU en pause (halt).
            self.current_threads[current_cpu_id] = None;
            println!("[scheduler] CPU {} has no work to do. Halting.", current_cpu_id);
            x86_64::instructions::hlt();
        }
    }

    /// Trouve le prochain thread à exécuter, en essayant d'abord la file locale,
    /// puis en volant du travail aux autres cœurs.
    fn find_next_thread(&self, current_cpu_id: u32) -> Option<ThreadId> {
        // 1. Essayer de prendre un thread depuis la file locale.
        if let Some(id) = self.ready_queues[current_cpu_id as usize].pop() {
            return Some(id);
        }

        // 2. Si la file locale est vide, voler du travail (work-stealing).
        // On parcourt les files des autres cœurs.
        for (cpu_id, queue) in self.ready_queues.iter().enumerate() {
            if cpu_id != current_cpu_id as usize {
                if let Some(id) = queue.pop() {
                    println!("[scheduler] CPU {} stole thread {} from CPU {}", current_cpu_id, id, cpu_id);
                    return Some(id);
                }
            }
        }

        // 3. Aucun travail trouvé nulle part.
        None
    }

    /// Marque le thread courant comme `Exited` et déclenche l'ordonnancement.
    pub fn exit_current_thread(&mut self) {
        let current_cpu_id = 0; // Simulation
        if let Some(id) = self.current_threads[current_cpu_id] {
            if let Some(thread) = self.threads.get(&id) {
                thread.lock().state = ThreadState::Exited;
            }
            // On ne remet pas le thread dans la file.
        }
        // Planifier le prochain thread. Cet appel ne reviendra pas pour le thread sortant.
        self.schedule();
    }
}

/// Fonction externe définie dans `context_switch.S`.
extern "C" {
    fn context_switch(old_context: *mut ThreadContext, new_rsp: u64);
}