//! # arch/x86_64/irq/mod.rs
//!
//! Routage IRQ GI-03 Driver Framework v10
//! Implémentation stricte : types canoniques + routage complet + watchdog

pub mod routing;
pub mod types;
pub mod watchdog;

// Réexporte les types publics
pub use types::{
    next_reg_id, IpcEndpoint, IrqAckResult, IrqError, IrqHandler, IrqOwnerPid, IrqRoute,
    IrqRouteRegistration, IrqSourceKind, IrqTable, IrqVector, IRQ_TABLE, MAX_HANDLERS_PER_IRQ,
    MAX_OVERFLOWS, MAX_PENDING_ACKS, SPIN_THRESHOLD,
};

// Réexporte les fonctions de routage
pub use routing::{
    ack_irq_canonical as ack_irq,
    ack_irq_syscall,
    dispatch_irq,
    irq_error_to_errno, // Helpers syscall
    parse_irq_source_kind,
    revoke_all_irq,
    sys_irq_register_canonical as sys_irq_register,
    sys_irq_register_syscall,
};
