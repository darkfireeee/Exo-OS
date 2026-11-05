//! # arch/x86_64/irq/watchdog.rs
//!
//! Periodic IRQ watchdog executor.
//! Single source: GI-03_Drivers_IRQ_DMA.md §3.4, FIX-77 v8, CORR-08, FIX-111
//!
//! Complete logic 0 stub:
//! Scan `IRQ_TABLE` -> Detect dropped IRQs (masked_since) -> Hard reset.

use crate::arch::x86_64::apic::{io_apic, local_apic};
use crate::arch::x86_64::irq::types::{
    IrqSourceKind, HARD_WATCHDOG_MS, IRQ_TABLE, SOFT_WATCHDOG_MS,
};
use crate::drivers::device_server_ipc;
use crate::scheduler::timer::clock::monotonic_ns;
use core::sync::atomic::Ordering;

#[inline]
fn clock_ms() -> u64 {
    monotonic_ns() / 1_000_000
}

/// watchdog_tick must be called periodically by the system timer tick (scheduler)
/// 10Hz or 100Hz max, to avoid impacting performance.
pub fn watchdog_tick() {
    let now = clock_ms();
    let table = IRQ_TABLE.read();

    for (_i, route_opt) in table.iter() {
        let Some(route) = route_opt else { continue };

        let pending = route.pending_acks.load(Ordering::Relaxed);
        if pending == 0 {
            // No IRQ pending -> no possible stall on this route.
            continue;
        }

        let masked_since = route.masked_since.load(Ordering::Relaxed);
        if masked_since == 0 {
            // Pending is > 0 but masked_since = 0.
            // This occurs transiently right before timestamping
            // in dispatch_irq, or if another thread has reset.
            continue;
        }

        let elapsed = now.saturating_sub(masked_since);

        // 1. Soft Alarm (Warning)
        if elapsed > SOFT_WATCHDOG_MS && !route.soft_alarmed.swap(true, Ordering::Relaxed) {
            log::warn!(
                "IRQ {} ({:?}) soft watchdog ({} ms), pending_acks={}",
                route.irq_line.as_u8(),
                route.source_kind,
                elapsed,
                pending
            );
        }

        // 2. Hard Alarm (Safety reset - FIX-77)
        if elapsed > HARD_WATCHDOG_MS {
            log::error!(
                "IRQ {} ({:?}) hard watchdog ({} ms) → force reset",
                route.irq_line.as_u8(),
                route.source_kind,
                elapsed
            );

            // FIX-77 v8: increment dispatch_generation BEFORE resetting pending,
            // to reject all in-flight ACKs from the dead wave.
            route.dispatch_generation.fetch_add(1, Ordering::AcqRel);

            // Reset to inactive state
            route.pending_acks.store(0, Ordering::Release);
            route.handled_count.store(0, Ordering::Release);
            route.masked_since.store(0, Ordering::Release);
            route.soft_alarmed.store(false, Ordering::Release);

            // For level-triggered IOAPIC, we must reinject a LAPIC EOI
            // that might be stuck by APIC hardware, then unmask.
            if route.source_kind == IrqSourceKind::IoApicLevel {
                route.masked.store(false, Ordering::Release);
                local_apic::eoi();
                if let Some(gsi) = route.gsi {
                    io_apic::unmask_irq(gsi);
                }
            }

            device_server_ipc::notify_driver_stall(route.irq_line.as_u8());
        }
    }
}
