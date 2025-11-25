//! Context switch implementation

pub mod windowed;
pub mod fpu;
pub mod simd;
pub mod benchmark;

pub use windowed::{switch, switch_full, switch_to, init_context, init};
// FPU/SIMD are stubs for now
pub use fpu::{save_fpu_state, restore_fpu_state};
pub use simd::{save_simd_state, restore_simd_state};
pub use benchmark::{benchmark_switch, SwitchBenchmark};

use crate::scheduler::thread::ThreadContext;

/// Context switch statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct SwitchStats {
    pub total_switches: u64,
    pub total_cycles: u64,
    pub min_cycles: u64,
    pub max_cycles: u64,
}

impl SwitchStats {
    pub fn average_cycles(&self) -> u64 {
        if self.total_switches == 0 {
            0
        } else {
            self.total_cycles / self.total_switches
        }
    }
    
    pub fn record_switch(&mut self, cycles: u64) {
        self.total_switches += 1;
        self.total_cycles += cycles;
        
        if self.min_cycles == 0 || cycles < self.min_cycles {
            self.min_cycles = cycles;
        }
        
        if cycles > self.max_cycles {
            self.max_cycles = cycles;
        }
    }
}
