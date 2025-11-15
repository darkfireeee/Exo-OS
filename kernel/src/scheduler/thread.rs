//! # Structure de Thread (TCB - Thread Control Block)
//!
//! Ce fichier définit la structure représentant un thread. Un thread est une unité
//! d'exécution avec son propre contexte (registres, pile) et son état.

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::VirtAddr;
use crate::println;
use alloc::alloc::{alloc, dealloc, Layout};

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
#[repr(C, align(16))]
pub struct ThreadContext {
    /// Pointeur de sommet de pile (RSP).
    /// C'est le seul champ que le code assembleur modifie directement.
    rsp: VirtAddr,
    
    /// Padding pour forcer l'alignement à 16 bytes
    _padding: u64,
}

impl ThreadContext {
    /// Crée un nouveau contexte avec une adresse de pile donnée.
    pub fn new(stack_top: VirtAddr) -> Self {
        Self { 
            rsp: stack_top,
            _padding: 0,
        }
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
#[repr(C, align(16))]
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
        // 1. Allouer une pile pour le thread depuis le heap global.
        // Taille par défaut: 16 KiB (un peu plus de marge pour appels imbriqués)
        let stack_size = 16 * 1024; // 16 KiB
        let layout = Layout::from_size_align(stack_size, 16).expect("Invalid stack layout");
        let raw_stack_ptr = alloc(layout);
        if raw_stack_ptr.is_null() {
            panic!("[thread] Échec allocation pile {} bytes", stack_size);
        }
        let stack_start = VirtAddr::from_ptr(raw_stack_ptr);
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
        // Libérer la pile
        let layout = Layout::from_size_align(self.stack_size, 16).unwrap();
        unsafe { dealloc(self.stack_start.as_mut_ptr(), layout); }
        println!("[thread] Dropped thread '{}' (ID: {}) - stack freed", self.name.as_deref().unwrap_or("unnamed"), self.id);
    }
}