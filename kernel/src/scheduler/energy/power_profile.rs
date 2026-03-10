// kernel/src/scheduler/energy/power_profile.rs
//
// Profil de puissance — table .rodata de coût énergétique par P-state/C-state.


/// Coût énergétique normalisé par P-state (P0 = 1000 = référence).
/// Valeurs indicatives ; calibrage réel via RAPL/ACPI.
#[link_section = ".rodata"]
static PSTATE_POWER_TABLE: [u16; 16] = [
    1000, 850, 720, 620, 530, 450, 380, 320,
    270,  220, 180, 140, 110,  80,  55,  30,
];

/// Économie normalisée par C-state (C0 = 0 = aucune économie).
#[link_section = ".rodata"]
static CSTATE_SAVINGS_TABLE: [u16; 4] = [0, 100, 350, 700];

/// Retourne le coût énergétique normalisé du P-state `p` (P0 = 1000).
pub fn pstate_power(p: usize) -> u16 {
    if p < 16 { PSTATE_POWER_TABLE[p] } else { PSTATE_POWER_TABLE[15] }
}

/// Retourne l'économie normalisée du C-state `c`.
pub fn cstate_savings(c: usize) -> u16 {
    if c < 4 { CSTATE_SAVINGS_TABLE[c] } else { CSTATE_SAVINGS_TABLE[3] }
}

/// Score énergétique global estimé : plus bas = plus économique.
/// Utilisé par le load balancer pour préférer des CPUs basse fréquence
/// quand la charge est faible.
pub fn energy_score(pstate: usize, cstate: usize) -> u32 {
    pstate_power(pstate) as u32 * 10
        - cstate_savings(cstate) as u32
}
