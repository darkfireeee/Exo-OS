// kernel/src/scheduler/energy/power_profile.rs
//
// Power profile — .rodata table of energy cost per P-state/C-state.

/// Normalized energy cost per P-state (P0 = 1000 = reference).
/// Indicative values; real calibration via RAPL/ACPI.
#[link_section = ".rodata"]
static PSTATE_POWER_TABLE: [u16; 16] = [
    1000, 850, 720, 620, 530, 450, 380, 320, 270, 220, 180, 140, 110, 80, 55, 30,
];

/// Normalized savings per C-state (C0 = 0 = no savings).
#[link_section = ".rodata"]
static CSTATE_SAVINGS_TABLE: [u16; 4] = [0, 100, 350, 700];

/// Returns normalized energy cost of P-state `p` (P0 = 1000).
pub fn pstate_power(p: usize) -> u16 {
    if p < 16 {
        PSTATE_POWER_TABLE[p]
    } else {
        PSTATE_POWER_TABLE[15]
    }
}

/// Returns normalized savings of C-state `c`.
pub fn cstate_savings(c: usize) -> u16 {
    if c < 4 {
        CSTATE_SAVINGS_TABLE[c]
    } else {
        CSTATE_SAVINGS_TABLE[3]
    }
}

/// Estimated global energy score: lower = more power-efficient.
/// Used by the load balancer to prefer low-frequency CPUs
/// when load is low.
pub fn energy_score(pstate: usize, cstate: usize) -> u32 {
    pstate_power(pstate) as u32 * 10 - cstate_savings(cstate) as u32
}
