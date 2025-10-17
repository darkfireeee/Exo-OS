//! # Structure de Thread (TCB - Thread Control Block)
//!
//! Ce fichier définit la structure représentant un thread. Un thread est une unité
//! d'exécution avec son propre contexte (registres, pile) et son état.

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::VirtAddr;
use crate::println;

/// Identifiant unique pour un thread.
pub type ThreadId = u64;

/// Compteur atomique pour générer des IDs de thread uniques.
static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);

/// États possibles d'un thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// Le thread est en cours d'exécution sur un CPU.
    Running,
    /// Le thread est prêt à être exécuté et attend dans une file.
    Ready,
    /// Le thread est bloqué, en attente d'un événement (I/O, sémaphore, etc.).
    Blocked,
    /// Le thread a terminé son exécution.
    Exited,
}

/// Contexte d'exécution d'un thread.
/// Cette structure est directement manipulée par le code assembleur `context_switch.S`.
/// Elle doit être `#[repr(C)]` pour garantir une disposition mémoire prévisible.
#[repr(C)]
pub struct ThreadContext {
    /// Pointeur de sommet de pile (RSP).
    /// C'est le seul champ que le code assembleur modifie directement.
    rsp: VirtAddr,
}

impl ThreadContext {
    /// Crée un nouveau contexte avec une adresse de pile donnée.
    pub fn new(stack_top: VirtAddr) -> Self {
        Self { rsp: stack_top }
    }

    /// Retourne l'adresse de sommet de pile.
    pub fn rsp(&self) -> VirtAddr {
        self.rsp
    }

    /// Met à jour l'adresse de sommet de pile.
    pub fn set_rsp(&mut self, rsp: VirtAddr) {
        self.rsp = rsp;
    }
}

/// La structure de contrôle de thread (TCB).
pub struct Thread {
    /// Identifiant unique du thread.
    pub id: ThreadId,
    /// Nom optionnel du thread, utile pour le débogage.
    pub name: Option<String>,
    /// État actuel du thread.
    pub state: ThreadState,
    /// Contexte d'exécution (registres sauvegardés).
    pub context: ThreadContext,
    /// Pointeur de début de la pile du thread.
    pub stack_start: VirtAddr,
    /// Taille de la pile du thread.
    pub stack_size: usize,
    /// Affinité de cœur CPU (optionnel).
    pub cpu_affinity: Option<u32>,
}

impl Thread {
    /// Crée un nouveau thread.
    ///
    /// # Arguments
    /// * `f` - La fonction que le thread exécutera.
    /// * `name` - Un nom optionnel pour le thread.
    /// * `cpu_affinity` - Le cœur CPU préféré.
    ///
    /// # Safety
    /// Cette fonction est `unsafe` car elle manipule directement la mémoire
    /// pour allouer une pile et préparer le contexte d'exécution initial.
    pub unsafe fn new(f: fn(), name: Option<&str>, cpu_affinity: Option<u32>) -> Self {
        // 1. Allouer une pile pour le thread.
        // Une taille de 8 KiB est un bon point de départ pour les threads du noyau.
        let stack_size = 8 * 1024; // 8 KiB
        
        // Pour l'instant, on utilise une allocation statique simple
        // TODO: Utiliser un vrai allocateur de pile quand le heap sera prêt
        static mut STACK_SPACE: [u8; 8192] = [0; 8192];
        let stack_start = VirtAddr::from_ptr(&STACK_SPACE as *const _);
        let stack_top: VirtAddr = stack_start + stack_size;

        // 2. Préparer la pile pour le premier lancement.
        // La pile doit ressembler à ce que `context_switch` s'attend à trouver
        // lorsqu'il restaure un contexte.
        // On y place l'adresse de la fonction `f` à exécuter.
        let stack_ptr = stack_top.as_mut_ptr::<VirtAddr>();
        stack_ptr.sub(1).write(VirtAddr::new(f as u64));

        // Le pointeur de pile initial pointe juste avant l'adresse de la fonction.
        let initial_rsp = VirtAddr::new(stack_ptr.sub(1) as u64);

        // 3. Créer la structure du thread.
        let thread = Self {
            id: NEXT_THREAD_ID.fetch_add(1, Ordering::SeqCst),
            name: name.map(|s| String::from(s)),
            state: ThreadState::Ready,
            context: ThreadContext::new(initial_rsp),
            stack_start,
            stack_size,
            cpu_affinity,
        };

        println!("[thread] Created thread '{}' (ID: {})", thread.name.as_deref().unwrap_or("unnamed"), thread.id);
        thread
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        // TODO: Libérer la mémoire de la pile quand le heap sera implémenté
        println!("[thread] Dropped thread '{}' (ID: {})", self.name.as_deref().unwrap_or("unnamed"), self.id);
    }
}