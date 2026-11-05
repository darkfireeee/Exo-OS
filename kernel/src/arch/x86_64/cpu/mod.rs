//! # arch/x86_64/cpu — CPU Submodule
//!
//! This module groups x86_64 CPU hardware primitives.
//! - `features` : CPUID detection, feature flags
//! - `msr`      : reading/writing MSRs (Model-Specific Registers)
//! - `fpu`      : raw ASM instructions XSAVE/XRSTOR/FXSAVE
//! - `tsc`      : TSC calibration, rdtsc wrapper
//! - `topology` : CPU topology (cores, HT, NUMA)
//!
//! ⚠️ This module does NOT contain FPU state logic (→ scheduler/fpu/)

pub mod features;
pub mod fpu;
pub mod msr;
pub mod topology;
pub mod tsc;
