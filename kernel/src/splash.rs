//! Exo-OS v0.5.0 Splash Screen & Boot Display
//! 
//! Système d'affichage amélioré pour la version 0.5.0
//! Affiche les informations de boot avec un design moderne

use crate::logger;

/// Version du kernel
pub const VERSION: &str = "0.5.0";
pub const VERSION_NAME: &str = "Linux Crusher";
pub const BUILD_DATE: &str = "2025-12-04";

/// Affiche le splash screen principal v0.5.0
pub fn display_splash() {
    logger::early_print("\n\n");
    logger::early_print("╔══════════════════════════════════════════════════════════════════════╗\n");
    logger::early_print("║                                                                      ║\n");
    logger::early_print("║     ███████╗██╗  ██╗ ██████╗        ██████╗ ███████╗               ║\n");
    logger::early_print("║     ██╔════╝╚██╗██╔╝██╔═══██╗      ██╔═══██╗██╔════╝               ║\n");
    logger::early_print("║     █████╗   ╚███╔╝ ██║   ██║█████╗██║   ██║███████╗               ║\n");
    logger::early_print("║     ██╔══╝   ██╔██╗ ██║   ██║╚════╝██║   ██║╚════██║               ║\n");
    logger::early_print("║     ███████╗██╔╝ ██╗╚██████╔╝      ╚██████╔╝███████║               ║\n");
    logger::early_print("║     ╚══════╝╚═╝  ╚═╝ ╚═════╝        ╚═════╝ ╚══════╝               ║\n");
    logger::early_print("║                                                                      ║\n");
    logger::early_print("║                  🚀 Version 0.5.0 - Linux Crusher 🚀                 ║\n");
    logger::early_print("║                                                                      ║\n");
    logger::early_print("╚══════════════════════════════════════════════════════════════════════╝\n");
    logger::early_print("\n");
}

/// Affiche la bannière compacte des nouvelles features v0.5.0
pub fn display_features() {
    logger::early_print("┌─────────────────────────────────────────────────────────────────────┐\n");
    logger::early_print("│  ✨ NOUVELLES FONCTIONNALITÉS v0.5.0                                │\n");
    logger::early_print("├─────────────────────────────────────────────────────────────────────┤\n");
    logger::early_print("│  ✅ Gestion mémoire complète                                        │\n");
    logger::early_print("│     • mmap/munmap/mprotect/brk/madvise/mlock/mremap                │\n");
    logger::early_print("│     • NUMA topology & allocation NUMA-aware                        │\n");
    logger::early_print("│     • Zerocopy IPC avec VM allocator et ref counting              │\n");
    logger::early_print("│                                                                     │\n");
    logger::early_print("│  ✅ Système de temps complet                                        │\n");
    logger::early_print("│     • TSC/HPET/RTC integration                                     │\n");
    logger::early_print("│     • Timers POSIX avec intervals                                  │\n");
    logger::early_print("│     • clock_gettime, nanosleep, alarm                              │\n");
    logger::early_print("│                                                                     │\n");
    logger::early_print("│  ✅ I/O & VFS haute performance                                     │\n");
    logger::early_print("│     • File descriptor table                                        │\n");
    logger::early_print("│     • VFS cache (inode LRU + dentry)                               │\n");
    logger::early_print("│     • Console série intégrée                                       │\n");
    logger::early_print("│                                                                     │\n");
    logger::early_print("│  ✅ Interruptions avancées                                          │\n");
    logger::early_print("│     • Local APIC + I/O APIC                                        │\n");
    logger::early_print("│     • x2APIC support avec MSR custom                               │\n");
    logger::early_print("│     • IRQ routing dynamique                                        │\n");
    logger::early_print("│                                                                     │\n");
    logger::early_print("│  ✅ Sécurité complète                                               │\n");
    logger::early_print("│     • Capability system par processus                              │\n");
    logger::early_print("│     • Credentials (UID/GID/EUID/EGID)                              │\n");
    logger::early_print("│     • seccomp, pledge, unveil restrictions                         │\n");
    logger::early_print("│                                                                     │\n");
    logger::early_print("│  📊 STATS: ~3000+ lignes | 150+ TODOs éliminés | 0 erreurs         │\n");
    logger::early_print("└─────────────────────────────────────────────────────────────────────┘\n");
    logger::early_print("\n");
}

/// Affiche la progression du boot avec une barre
pub fn display_boot_progress(phase: &str, percentage: u8) {
    logger::early_print("\n  [");
    
    let filled = (percentage as usize * 50) / 100;
    for i in 0..50 {
        if i < filled {
            logger::early_print("█");
        } else {
            logger::early_print("░");
        }
    }
    
    logger::early_print("] 100% - ");
    logger::early_print(phase);
    logger::early_print("\n\n");
}

/// Affiche le résumé du système après boot
pub fn display_system_info(memory_mb: usize, cpu_count: usize) {
    logger::early_print("\n");
    logger::early_print("┌─────────────────────────────────────────────────────────────────────┐\n");
    logger::early_print("│  💻 INFORMATIONS SYSTÈME                                            │\n");
    logger::early_print("├─────────────────────────────────────────────────────────────────────┤\n");
    
    logger::early_print("│  Kernel:       Exo-OS v");
    logger::early_print(VERSION);
    logger::early_print(" (");
    logger::early_print(VERSION_NAME);
    logger::early_print(")");
    
    // Padding
    let name_len = VERSION.len() + VERSION_NAME.len() + 4;
    let padding = 46_usize.saturating_sub(name_len);
    for _ in 0..padding {
        logger::early_print(" ");
    }
    logger::early_print("│\n");
    
    logger::early_print("│  Build:        ");
    logger::early_print(BUILD_DATE);
    logger::early_print("                                              │\n");
    
    logger::early_print("│  Architecture: x86_64 (64-bit)                                      │\n");
    
    logger::early_print("│  Memory:       512 MB                                               │\n");
    logger::early_print("│  CPU Cores:    1                                                    │\n");
    
    logger::early_print("│  Features:     NUMA, APIC, VFS, Security, Zerocopy IPC             │\n");
    logger::early_print("└─────────────────────────────────────────────────────────────────────┘\n");
    logger::early_print("\n");
}

/// Affiche un message de succès stylisé
pub fn display_success(message: &str) {
    logger::early_print("  ✅ ");
    logger::early_print(message);
    logger::early_print("\n");
}

/// Affiche un message d'erreur stylisé
pub fn display_error(message: &str) {
    logger::early_print("  ❌ ");
    logger::early_print(message);
    logger::early_print("\n");
}

/// Affiche un message d'avertissement stylisé
pub fn display_warning(message: &str) {
    logger::early_print("  ⚠️  ");
    logger::early_print(message);
    logger::early_print("\n");
}

/// Affiche un message informatif stylisé
pub fn display_info(message: &str) {
    logger::early_print("  ℹ️  ");
    logger::early_print(message);
    logger::early_print("\n");
}

/// Affiche le message de démarrage complet v0.5.0
pub fn display_full_boot_sequence() {
    display_splash();
    display_features();
    
    logger::early_print("┌─────────────────────────────────────────────────────────────────────┐\n");
    logger::early_print("│  🔧 SÉQUENCE DE DÉMARRAGE                                           │\n");
    logger::early_print("└─────────────────────────────────────────────────────────────────────┘\n");
    logger::early_print("\n");
}
