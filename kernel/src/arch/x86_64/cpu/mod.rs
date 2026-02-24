//! # arch/x86_64/cpu — Sous-module CPU
//!
//! Ce module regroupe les primitives matérielles CPU x86_64.
//! - `features` : détection CPUID, feature flags
//! - `msr`      : lecture/écriture MSR (Model-Specific Registers)
//! - `fpu`      : instructions ASM brutes XSAVE/XRSTOR/FXSAVE
//! - `tsc`      : TSC calibration, rdtsc wrapper
//! - `topology` : topologie CPU (cores, HT, NUMA)
//!
//! ⚠️ Ce module NE contient PAS la logique d'état FPU (→ scheduler/fpu/)

pub mod features;
pub mod fpu;
pub mod msr;
pub mod topology;
pub mod tsc;
