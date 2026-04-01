//! # arch/x86_64/irq/mod.rs
//!
//! Routage IRQ GI-03 Driver Framework v10
//! Implémentation stricte : types canoniques + routage complet + watchdog

pub mod types;
pub mod routing;
pub mod watchdog;

// Réexporte les types publics
pub use types::{
    IrqVector, IrqOwnerPid, IrqSourceKind, IrqRouteRegistration, IrqError, IrqHandler, IrqAckResult,
    IrqRoute, IrqTable, IRQ_TABLE, next_reg_id, IpcEndpoint,
    MAX_HANDLERS_PER_IRQ, MAX_PENDING_ACKS, MAX_OVERFLOWS, SPIN_THRESHOLD,      
};

// Réexporte les fonctions de routage
pub use routing::{
    sys_irq_register_syscall as sys_irq_register,  // Alias pour compatibilité syscall
    ack_irq_syscall as ack_irq,  // Version syscall simplifiée
    revoke_all_irq, dispatch_irq,
    parse_irq_source_kind, irq_error_to_errno,  // Helpers syscall
};
