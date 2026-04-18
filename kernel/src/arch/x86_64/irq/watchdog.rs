//! # arch/x86_64/irq/watchdog.rs
//!
//! Exécuteur de Watchdog périodique pour les IRQs.
//! Source unique : GI-03_Drivers_IRQ_DMA.md §3.4, FIX-77 v8, CORR-08, FIX-111
//!
//! Logique complète 0 stub :
//! Scan de `IRQ_TABLE` -> Détection d'IRQ droppées (masked_since) -> Reset dur.

use core::sync::atomic::Ordering;
use crate::arch::x86_64::irq::types::{
    IRQ_TABLE, SOFT_WATCHDOG_MS, HARD_WATCHDOG_MS, IrqSourceKind,
};
use crate::arch::x86_64::apic::{io_apic, local_apic};
use crate::drivers::device_server_ipc;
use crate::scheduler::timer::clock::monotonic_ns;

#[inline]
fn clock_ms() -> u64 {
    monotonic_ns() / 1_000_000
}

/// watchdog_tick doit être appelé périodiquement par le timer tick system (scheduler)
/// 10Hz ou 100Hz max, pour éviter d'impacter les performances.
pub fn watchdog_tick() {
    let now = clock_ms();
    let table = IRQ_TABLE.read();

    for (_i, route_opt) in table.iter() {
        let Some(route) = route_opt else { continue };

        let pending = route.pending_acks.load(Ordering::Relaxed);
        if pending == 0 {
            // Pas d'IRQ en attente -> pas de blocage possible sur cette route.
            continue;
        }

        let masked_since = route.masked_since.load(Ordering::Relaxed);
        if masked_since == 0 {
            // Pending est > 0 mais masked_since = 0.
            // Cela survient de façon transitoire juste avant le timestamping
            // dans dispatch_irq, ou si un autre thread a reset.
            continue;
        }

        let elapsed = now.saturating_sub(masked_since);

        // 1. Soft Alarm (Avertissement)
        if elapsed > SOFT_WATCHDOG_MS && !route.soft_alarmed.swap(true, Ordering::Relaxed) {
            log::warn!(
                "IRQ {} ({:?}) soft watchdog ({} ms), pending_acks={}",
                route.irq_line.as_u8(), route.source_kind, elapsed, pending
            );
        }

        // 2. Hard Alarm (Reset de sécurité - FIX-77)
        if elapsed > HARD_WATCHDOG_MS {
            log::error!(
                "IRQ {} ({:?}) hard watchdog ({} ms) → force reset",
                route.irq_line.as_u8(), route.source_kind, elapsed
            );

            // FIX-77 v8 : incrémenter dispatch_generation AVANT de reset pending,
            // pour rejeter tous les ACKs volants de la vague morte.
            route.dispatch_generation.fetch_add(1, Ordering::AcqRel);
            
            // Remise à l'état inactif
            route.pending_acks.store(0, Ordering::Release);
            route.handled_count.store(0, Ordering::Release);
            route.masked_since.store(0, Ordering::Release);
            route.soft_alarmed.store(false, Ordering::Release);

            // Pour l'IOAPIC Level-triggered, on doit réinjecter un EOI LAPIC
            // qui est peut-être coincé par le hardware APIC, puis démasquer.
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
