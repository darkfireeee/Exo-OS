// kernel/src/process/lifecycle/exec.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// execve() — Remplacement de l'image processus (Exo-OS Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE PROC-01 : process/ (couche 1.5) ne peut pas importer fs/ (couche 3).
// SOLUTION : trait ElfLoader enregistré par fs/ au boot.
//
// Séquence do_execve() :
//   1. Vérifier les permissions (creds).
//   2. Appeler ElfLoader::load_elf() pour charger le binaire.
//   3. Fermer les fds O_CLOEXEC.
//   4. Réinitialiser l'espace d'adressage virtuel.
//   5. Mettre à jour le TCB (entry_point, initial_rsp, TLS).
//   6. Mettre à jour le PCB (flags, compteurs).
//   7. Réinitialiser les signaux (handlers → SIG_DFL).
//   8. Retourner vers userspace.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::Ordering;
use spin::Once;
use crate::process::core::tcb::{ProcessThread, ThreadAddress};
use crate::process::core::pcb::{ProcessControlBlock, ProcessState, process_flags};
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::process::signal::mask::reset_signals_on_exec;
use crate::process::signal::mask::block_all_except_kill;

// ─────────────────────────────────────────────────────────────────────────────
// Trait ElfLoader — RÈGLE PROC-01 : abstraction de la couche fs/
// ─────────────────────────────────────────────────────────────────────────────

/// Paramètres issus du chargement ELF.
#[derive(Debug, Clone)]
pub struct ElfLoadResult {
    /// Adresse d'entrée (entry point du binaire).
    pub entry_point:       u64,
    /// Pointeur de pile initial (RSP au démarrage).
    pub initial_stack_top: u64,
    /// Base de la TLS statique chargée (segment .tdata/.tbss).
    pub tls_base:          u64,
    /// Taille de la TLS statique.
    pub tls_size:          usize,
    /// Adresse de début du heap brk (juste après le segment .bss).
    pub brk_start:         u64,
    /// Adresse physique (CR3) du nouvel espace d'adressage créé.
    pub cr3:               u64,
    /// Pointeur opaque vers le UserAddressSpace créé par fs/.
    pub addr_space_ptr:    usize,
}

/// Erreurs renvoyées par ElfLoader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfLoadError {
    /// Fichier non trouvé.
    NotFound,
    /// Format ELF invalide ou corrompu.
    InvalidElf,
    /// Permission refusée.
    PermissionDenied,
    /// Pas assez de mémoire pour charger.
    OutOfMemory,
    /// Architecture non supportée.
    UnsupportedArch,
    /// Interprète (PT_INTERP) non trouvé.
    InterpreterNotFound,
}

/// Erreurs d'execve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecError {
    /// ELF non chargeable (wrapper).
    ElfLoadFailed(ElfLoadError),
    /// SCM / capability refusée.
    PermissionDenied,
    /// Trop d'arguments (E2BIG).
    ArgListTooLong,
    /// Nom de fichier trop long.
    NameTooLong,
    /// Processus déjà en train de quitter.
    ProcessExiting,
    /// Aucun ElfLoader enregistré.
    NoLoader,
}

/// Trait que `fs/` doit implémenter et enregistrer via `register_elf_loader()`.
///
/// La couche process/ ne connaît PAS les types fs/  — uniquement ce trait.
pub trait ElfLoader: Send + Sync {
    /// Charge un ELF depuis le chemin donné dans l'espace d'adressage courant.
    ///
    /// # Arguments
    /// * `path`   — chemin absolu dans le VFS.
    /// * `argv`   — vecteur d'arguments (argv[0] = binaire).
    /// * `envp`   — variables d'environnement.
    /// * `cr3_in` — CR3 de l'espace d'adressage existant à réinitialiser.
    ///
    /// # Returns
    /// `ElfLoadResult` décrivant l'espace d'adressage peuplé.
    fn load_elf(
        &self,
        path:   &str,
        argv:   &[&str],
        envp:   &[&str],
        cr3_in: u64,
    ) -> Result<ElfLoadResult, ElfLoadError>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Registre global du chargeur ELF
// ─────────────────────────────────────────────────────────────────────────────

static ELF_LOADER: Once<&'static dyn ElfLoader> = Once::new();

/// Enregistre le chargeur ELF (appelé une seule fois depuis fs/ au boot).
///
/// # Safety
/// `loader` doit avoir une durée de vie `'static` (typiquement une référence
/// à un objet alloué statiquement dans fs/).
pub fn register_elf_loader(loader: &'static dyn ElfLoader) {
    ELF_LOADER.call_once(|| loader);
}

// ─────────────────────────────────────────────────────────────────────────────
// do_execve — implémentation principale
// ─────────────────────────────────────────────────────────────────────────────

/// Execute un nouveau binaire ELF dans le contexte du thread courant.
///
/// Cette fonction est appelée depuis le syscall execve() après validation
/// des paramètres utilisateur.
///
/// # Safety
/// `thread` doit pointer vers le ProcessThread du thread appelant.
/// L'espace d'adressage sera reconstruit — aucun accès userspace après l'appel.
pub fn do_execve(
    thread: &mut ProcessThread,
    pcb:    &ProcessControlBlock,
    path:   &str,
    argv:   &[&str],
    envp:   &[&str],
) -> Result<(), ExecError> {
    // Vérifier que le processus n'est pas en train de quitter.
    if pcb.is_exiting() {
        return Err(ExecError::ProcessExiting);
    }

    // Valider la longueur du chemin et des arguments.
    if path.len() > 4096 {
        return Err(ExecError::NameTooLong);
    }
    let total_arg_len: usize = argv.iter().map(|s| s.len() + 1).sum::<usize>()
        + envp.iter().map(|s| s.len() + 1).sum::<usize>();
    if total_arg_len > 128 * 1024 {
        return Err(ExecError::ArgListTooLong);
    }

    // Obtenir le chargeur ELF (enregistré depuis fs/ au boot).
    let loader = ELF_LOADER.get().ok_or(ExecError::NoLoader)?;

    // Étape 3 (LAC-08 / PROC-03) : bloquer TOUS les signaux sauf SIGKILL/SIGSTOP
    // AVANT le chargement ELF. Empêche un signal livré entre load_elf() et
    // reset_signals_on_exec() d'invoquer l'ancien handler dans un adress space
    // partiellement remplacé. reset_signals_on_exec() débloque ultérieurement.
    block_all_except_kill(&thread.sched_tcb);

    // Charger le nouveau binaire dans l'espace d'adressage.
    let cr3_current = thread.sched_tcb.cr3;
    let elf_result = loader
        .load_elf(path, argv, envp, cr3_current)
        .map_err(ExecError::ElfLoadFailed)?;

    // Fermer les fds O_CLOEXEC.
    let closed_handles = {
        let mut files = pcb.files.lock();
        files.close_on_exec()
    };
    // Notifier fs/ de la fermeture (via handle opaque — pas d'import fs/).
    // NOTE: les handles sont simplement abandonnés ; fs/ les collectera via
    // un mécanisme de GC de handles (hors scope de ce module).
    drop(closed_handles);

    // Réinitialiser les signaux (handlers → SIG_DFL, masque → 0).
    reset_signals_on_exec(&thread.sched_tcb);

    // Mettre à jour l'espace d'adressage dans le TCB scheduler.
    thread.sched_tcb.cr3 = elf_result.cr3;

    // Mettre à jour les adresses du thread.
    thread.addresses = ThreadAddress {
        stack_base:       0,  // fourni par ELF_LOADER dans initial_stack_top
        stack_size:       0,
        entry_point:      elf_result.entry_point,
        initial_rsp:      elf_result.initial_stack_top,
        tls_base:         elf_result.tls_base,
        pthread_ptr:      0,
        sigaltstack_base: 0,
        sigaltstack_size: 0,
    };
    thread.tls_gs_base.store(elf_result.tls_base, Ordering::Release);
    thread.tls_size = elf_result.tls_size;

    // Mettre à jour le PCB avec le nouvel espace d'adressage.
    pcb.cr3.store(elf_result.cr3, Ordering::Release);
    pcb.address_space.store(elf_result.addr_space_ptr, Ordering::Release);
    pcb.brk_start.store(elf_result.brk_start, Ordering::Release);
    pcb.brk_current.store(elf_result.brk_start, Ordering::Release);

    // Marquer EXEC_DONE et retirer FORKED.
    pcb.flags.fetch_or(process_flags::EXEC_DONE, Ordering::Release);
    pcb.flags.fetch_and(!process_flags::FORKED, Ordering::Release);

    // Transition vers Running.
    pcb.set_state(ProcessState::Running);

    Ok(())
}
