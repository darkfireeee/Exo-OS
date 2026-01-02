//! Thread Migration via IPI
//!
//! Phase 2d: Cross-CPU thread migration using Inter-Processor Interrupts
//!
//! When a thread's CPU affinity changes, we need to migrate it to the target CPU.
//! This is done via IPI to avoid cross-CPU lock contention.

use crate::arch::x86_64::interrupts::ipi::{send_ipi, IPI_RESCHEDULE_VECTOR};
use crate::scheduler::thread::{ThreadId, Thread};
use crate::scheduler::smp_init::current_cpu_id;
use alloc::sync::Arc;
use alloc::collections::VecDeque;
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Maximum pending migrations per CPU
const MAX_PENDING_MIGRATIONS: usize = 64;

/// Migration request
#[derive(Debug, Clone)]
pub struct MigrationRequest {
    /// Thread to migrate
    pub thread: Arc<Thread>,
    
    /// Target CPU
    pub target_cpu: usize,
    
    /// Source CPU (for statistics)
    pub source_cpu: usize,
}

/// Per-CPU migration queue
pub struct MigrationQueue {
    /// CPU ID
    cpu_id: usize,
    
    /// Pending migrations for this CPU
    pending: Mutex<VecDeque<MigrationRequest>>,
    
    /// Statistics
    migrations_in: AtomicUsize,
    migrations_out: AtomicUsize,
}

impl MigrationQueue {
    pub const fn new(cpu_id: usize) -> Self {
        Self {
            cpu_id,
            pending: Mutex::new(VecDeque::new()),
            migrations_in: AtomicUsize::new(0),
            migrations_out: AtomicUsize::new(0),
        }
    }
    
    /// Add migration request (called by source CPU)
    pub fn add_migration(&self, request: MigrationRequest) -> bool {
        let mut pending = self.pending.lock();
        
        if pending.len() >= MAX_PENDING_MIGRATIONS {
            crate::logger::warn(&alloc::format!(
                "[MIGRATE] Queue full on CPU {} (dropping migration)",
                self.cpu_id
            ));
            return false;
        }
        
        pending.push_back(request);
        self.migrations_in.fetch_add(1, Ordering::Relaxed);
        
        crate::logger::debug(&alloc::format!(
            "[MIGRATE] Added migration to CPU {} (queue len: {})",
            self.cpu_id,
            pending.len()
        ));
        
        true
    }
    
    /// Process pending migrations (called by IPI handler on target CPU)
    pub fn process_migrations(&self) {
        let mut pending = self.pending.lock();
        
        let count = pending.len();
        if count == 0 {
            return;
        }
        
        crate::logger::debug(&alloc::format!(
            "[MIGRATE] Processing {} migrations on CPU {}",
            count,
            self.cpu_id
        ));
        
        // Enqueue all migrated threads to this CPU's run queue
        while let Some(request) = pending.pop_front() {
            use crate::scheduler::core::percpu_queue::PER_CPU_QUEUES;
            
            if let Some(queue) = PER_CPU_QUEUES.get(self.cpu_id) {
                queue.enqueue(request.thread);
                
                crate::logger::debug(&alloc::format!(
                    "[MIGRATE] Thread migrated from CPU {} to CPU {}",
                    request.source_cpu,
                    self.cpu_id
                ));
            } else {
                crate::logger::error(&alloc::format!(
                    "[MIGRATE] Invalid target CPU: {}",
                    self.cpu_id
                ));
            }
        }
    }
    
    /// Get statistics
    pub fn stats(&self) -> (usize, usize) {
        (
            self.migrations_in.load(Ordering::Relaxed),
            self.migrations_out.load(Ordering::Relaxed),
        )
    }
}

/// Global migration queues (one per CPU)
pub static MIGRATION_QUEUES: MigrationQueues = MigrationQueues::new();

pub struct MigrationQueues {
    queues: [MigrationQueue; 8], // Max 8 CPUs
}

impl MigrationQueues {
    pub const fn new() -> Self {
        Self {
            queues: [
                MigrationQueue::new(0),
                MigrationQueue::new(1),
                MigrationQueue::new(2),
                MigrationQueue::new(3),
                MigrationQueue::new(4),
                MigrationQueue::new(5),
                MigrationQueue::new(6),
                MigrationQueue::new(7),
            ],
        }
    }
    
    pub fn get(&self, cpu_id: usize) -> Option<&MigrationQueue> {
        if cpu_id < self.queues.len() {
            Some(&self.queues[cpu_id])
        } else {
            None
        }
    }
}

/// Migrate thread to target CPU (via IPI)
///
/// # Arguments
/// * `thread` - Thread to migrate
/// * `target_cpu` - Target CPU ID
///
/// # Returns
/// true if migration initiated successfully
pub fn migrate_thread(thread: Arc<Thread>, target_cpu: usize) -> bool {
    let source_cpu = current_cpu_id();
    
    if source_cpu == target_cpu {
        // Already on target CPU, no migration needed
        crate::logger::debug("[MIGRATE] Thread already on target CPU");
        return true;
    }
    
    // Validate target CPU
    let num_cpus = crate::arch::x86_64::smp::get_cpu_count();
    if target_cpu >= num_cpus {
        crate::logger::error(&alloc::format!(
            "[MIGRATE] Invalid target CPU: {} (max: {})",
            target_cpu,
            num_cpus - 1
        ));
        return false;
    }
    
    crate::logger::info(&alloc::format!(
        "[MIGRATE] Migrating thread {} from CPU {} to CPU {}",
        thread.id(),
        source_cpu,
        target_cpu
    ));
    
    // Add to target CPU's migration queue
    let migration = MigrationRequest {
        thread,
        target_cpu,
        source_cpu,
    };
    
    if let Some(queue) = MIGRATION_QUEUES.get(target_cpu) {
        if !queue.add_migration(migration) {
            return false;
        }
    } else {
        crate::logger::error(&alloc::format!(
            "[MIGRATE] No migration queue for CPU {}",
            target_cpu
        ));
        return false;
    }
    
    // Send IPI to target CPU to process migration
    unsafe {
        let target_apic_id = get_apic_id_for_cpu(target_cpu);
        send_ipi(target_apic_id, IPI_RESCHEDULE_VECTOR);
        
        crate::logger::debug(&alloc::format!(
            "[MIGRATE] Sent IPI to CPU {} (APIC ID: {})",
            target_cpu,
            target_apic_id
        ));
    }
    
    true
}

/// Get APIC ID for a CPU
fn get_apic_id_for_cpu(cpu_id: usize) -> u32 {
    use crate::arch::x86_64::smp::SMP_SYSTEM;
    use core::sync::atomic::Ordering;
    
    if let Some(cpu_info) = SMP_SYSTEM.cpu(cpu_id) {
        cpu_info.apic_id.load(Ordering::Acquire) as u32
    } else {
        crate::logger::warn(&alloc::format!(
            "[MIGRATE] Unknown APIC ID for CPU {}, using ID as fallback",
            cpu_id
        ));
        cpu_id as u32
    }
}

/// Process migrations for current CPU (called from IPI handler)
pub fn process_current_cpu_migrations() {
    let cpu_id = current_cpu_id();
    
    if let Some(queue) = MIGRATION_QUEUES.get(cpu_id) {
        queue.process_migrations();
    }
}

/// Get migration statistics for all CPUs
pub fn migration_stats() -> alloc::vec::Vec<(usize, usize, usize)> {
    let mut stats = alloc::vec::Vec::new();
    let num_cpus = crate::arch::x86_64::smp::get_cpu_count();
    
    for cpu_id in 0..num_cpus {
        if let Some(queue) = MIGRATION_QUEUES.get(cpu_id) {
            let (migrations_in, migrations_out) = queue.stats();
            stats.push((cpu_id, migrations_in, migrations_out));
        }
    }
    
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_migration_queue() {
        use crate::scheduler::thread::Thread;
        use alloc::boxed::Box;
        
        let queue = MigrationQueue::new(1);
        
        // Create dummy thread
        let thread = Box::new(Thread::new_kernel(1, "test", || loop {}, 4096));
        let thread = Arc::new(*thread);
        
        let request = MigrationRequest {
            thread: thread.clone(),
            target_cpu: 1,
            source_cpu: 0,
        };
        
        assert!(queue.add_migration(request));
        
        let (in_count, _) = queue.stats();
        assert_eq!(in_count, 1);
    }
}
