//! # arch/x86_64/irq/mod.rs
//!
//! GI-03 Driver Framework v10 IRQ routing.
//! Strict implementation: canonical types + full routing + watchdog.

pub mod pic;
pub mod routing;
pub mod types;
pub mod watchdog;

// Re-export public types
pub use types::{
    next_reg_id, IpcEndpoint, IrqAckResult, IrqError, IrqHandler, IrqOwnerPid, IrqRoute,
    IrqRouteRegistration, IrqSourceKind, IrqTable, IrqVector, IRQ_TABLE, MAX_HANDLERS_PER_IRQ,
    MAX_OVERFLOWS, MAX_PENDING_ACKS, SPIN_THRESHOLD,
};

// Re-export routing functions
pub use routing::{
    ack_irq_canonical as ack_irq,
    ack_irq_syscall,
    dispatch_irq,
    irq_error_to_errno, // Syscall helpers
    parse_irq_source_kind,
    reset_all_masked_since,
    revoke_all_irq,
    sys_irq_register_canonical as sys_irq_register,
    sys_irq_register_syscall,
};
