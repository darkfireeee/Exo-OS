// kernel/src/arch/x86_64/time/calibration/cpuid_nominal.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Calibration TSC nominale via CPUID  — Intel leaf 0x15 / 0x16, AMD leaf 0x80000019
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Sources de fréquence nominale (sans timer externe)
//
//   CPUID leaf 0x15  (Intel Skylake+, 2015)
//     EAX = dénominateur ratio TSC/cristal
//     EBX = numérateur  ratio TSC/cristal
//     ECX = fréquence cristal en Hz (0 si non fournie → 24 MHz fallback Intel)
//     TSC_Hz = crystal_hz × EBX / EAX
//
//   CPUID leaf 0x16  (Intel Skylake+)
//     EAX[15:0] = fréquence de base CPU en MHz
//     Moins précis que 0x15, utilisé comme fallback CPUID ou cross-check.
//
//   CPUID leaf 0x80000019  (AMD Family 17h+, Zen)
//     CurrUcodeRev / Core Performance Boost → TSC nominal en MHz (bits 31:16 ECX)
//     Ou via MSR 0xC0010064 (P-state 0) → FID/DID → fréquence TSC
//
// ## Crystals Intel référence (ECX = 0 dans leaf 0x15)
//   Skylake / Kaby Lake / Coffee Lake   : 24 000 000 Hz
//   Atom Goldmont / Silvermont          : 19 200 000 Hz
//   Atom Gemini Lake / Tremont          : 19 200 000 Hz
//   Tiger Lake / Alder Lake             : 38 400 000 Hz (nouveaux SoC)
//   Latence MMIO ICH/FCH               : non concerné ici
//
// ## Rating : 150 — nomination fabricant, sans dérive mesurable
//   Avantage : pas besoin de HPET/PM Timer → boot très rapidement
//   Inconvénient : ne mesure pas la dérive réelle du TSC en environnement hostile
//     (VM, Turbo Boost désactivé, CPPC activé, etc.)
//
// ## Précision : ±0.01% si cristal référence fourni, ±1% si ECX=0 et fallback 24 MHz
// ════════════════════════════════════════════════════════════════════════════════


// ── Constantes cristaux de référence Intel ────────────────────────────────────

/// Fréquence cristal standard Intel 24 MHz (Skylake, Kaby Lake, Coffee Lake…)
pub const CRYSTAL_FREQ_24MHZ:  u64 = 24_000_000;
/// Fréquence cristal standard Intel 19.2 MHz (Atom Goldmont, Silvermont)
pub const CRYSTAL_FREQ_192MHZ: u64 = 19_200_000;
/// Fréquence cristal standard Intel 38.4 MHz (Tiger Lake, Alder Lake SoC)
pub const CRYSTAL_FREQ_384MHZ: u64 = 38_400_000;
/// Fréquence cristal AMD Zen (généralement 100 MHz / divider → TSC via MSRC001_00[6B:64])
pub const CRYSTAL_FREQ_100MHZ: u64 = 100_000_000;

/// Fréquence minimum acceptable pour un TSC nominal [100 MHz].
pub const TSC_MIN_HZ: u64 = 100_000_000;
/// Fréquence maximum acceptable pour un TSC nominal [10 GHz].
pub const TSC_MAX_HZ: u64 = 10_000_000_000;

// ── Résultats détaillés ────────────────────────────────────────────────────────

/// Résultat complet de la lecture CPUID leaf 0x15.
#[derive(Debug, Clone, Copy)]
pub struct CpuidLeaf15Result {
    /// Dénominateur du ratio cristal/TSC (EAX).
    pub denominator:  u32,
    /// Numérateur du ratio cristal/TSC (EBX).
    pub numerator:    u32,
    /// Fréquence cristal en Hz (ECX, 0 si non fournie).
    pub crystal_hz:   u32,
    /// Fréquence cristal effective utilisée (après fallback si ECX=0).
    pub crystal_used: u64,
    /// Fréquence TSC calculée, 0 si invalide.
    pub tsc_hz:       u64,
    /// Source du cristal utilisée.
    pub crystal_src:  CrystalSource,
}

/// Source de la fréquence cristal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrystalSource {
    /// ECX fourni par CPUID (le plus fiable).
    CpuidEcx,
    /// Valeur standard 24 MHz (Skylake, KBL, CFL déduite depuis CPUID brand).
    Standard24Mhz,
    /// Valeur standard 19.2 MHz (Atom Goldmont/Silvermont).
    Standard192Mhz,
    /// Valeur standard 38.4 MHz (Tiger Lake, Alder Lake).
    Standard384Mhz,
    /// Valeur par défaut Intel (24 MHz, pour CPUs non identifiés).
    DefaultIntel24Mhz,
    /// Leaf 0x15 non disponible.
    NotAvailable,
}

impl CrystalSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CrystalSource::CpuidEcx        => "CPUID-ECX",
            CrystalSource::Standard24Mhz   => "24MHz-SKL",
            CrystalSource::Standard192Mhz  => "19.2MHz-ATOM",
            CrystalSource::Standard384Mhz  => "38.4MHz-TGL",
            CrystalSource::DefaultIntel24Mhz => "24MHz-DFLT",
            CrystalSource::NotAvailable    => "N/A",
        }
    }
}

/// Résultat de la lecture CPUID leaf 0x16.
#[derive(Debug, Clone, Copy)]
pub struct CpuidLeaf16Result {
    /// Fréquence de base CPU en MHz (EAX[15:0]).
    pub base_mhz:   u32,
    /// Fréquence max (Turbo) en MHz (EBX[15:0]).
    pub max_mhz:    u32,
    /// Fréquence bus en MHz (ECX[15:0]).
    pub bus_mhz:    u32,
    /// Fréquence TSC déduite (= base_mhz × 1_000_000).
    pub tsc_hz:     u64,
}

// ── API principale ─────────────────────────────────────────────────────────────

/// Tente de lire la fréquence TSC nominale via CPUID leaf 0x15.
///
/// Algorithme :
///   1. Vérifier que leaf 0x15 est supportée (max_leaf >= 0x15)
///   2. Lire EAX (dénominateur), EBX (numérateur), ECX (crystal_hz)
///   3. Si ECX == 0 → détecter le cristal depuis le modèle CPU
///   4. Calculer : tsc_hz = crystal_hz × EBX / EAX
///   5. Valider la plage [100 MHz, 10 GHz]
///
/// RÈGLE CALIBRATION-CPUID-01 : Si EAX=0 ou EBX=0, leaf 0x15 invalide → None.
/// RÈGLE CALIBRATION-CPUID-02 : Si ECX=0, détecter le cristal par modèle CPU.
///
/// Retourne `None` si leaf 0x15 non disponible ou données invalides.
pub fn cpuid_tsc_hz() -> Option<u64> {
    let result = cpuid_leaf15_full()?;
    if result.tsc_hz == 0 { None } else { Some(result.tsc_hz) }
}

/// Variante retournant le résultat complet avec diagnostic.
pub fn cpuid_tsc_hz_detailed() -> Option<CpuidLeaf15Result> {
    cpuid_leaf15_full()
}

/// Tente de lire la fréquence de base via CPUID leaf 0x16.
///
/// Retourne la fréquence de base CPU × 1_000_000 (conversion MHz → Hz).
/// Précision ±1 MHz (arrondi au MHz fabricant).
pub fn cpuid_tsc_hz_leaf16() -> Option<u64> {
    let result = cpuid_leaf16_full()?;
    if result.tsc_hz == 0 { None } else { Some(result.tsc_hz) }
}

/// Variante retournant le résultat complet leaf 0x16.
pub fn cpuid_leaf16_detailed() -> Option<CpuidLeaf16Result> {
    cpuid_leaf16_full()
}

/// Vérifie si CPUID leaf 0x15 est disponible.
pub fn cpuid_leaf15_available() -> bool {
    max_cpuid_leaf() >= 0x15
}

/// Vérifie si CPUID leaf 0x16 est disponible.
pub fn cpuid_leaf16_available() -> bool {
    max_cpuid_leaf() >= 0x16
}

/// Retourne la meilleure estimation CPUID disponible (0x15 > 0x16 > None).
pub fn cpuid_best_estimate() -> Option<u64> {
    if let Some(hz) = cpuid_tsc_hz() {
        return Some(hz);
    }
    cpuid_tsc_hz_leaf16()
}

// ── Implémentation interne leaf 0x15 ─────────────────────────────────────────

/// Lit CPUID leaf 0x15 et calcule la fréquence TSC complète.
fn cpuid_leaf15_full() -> Option<CpuidLeaf15Result> {
    if max_cpuid_leaf() < 0x15 {
        return None;
    }

    let (eax, ebx, ecx) = read_cpuid_leaf_15();

    // EAX ou EBX nul = leaf invalide ou non implémentée sur ce CPU.
    if eax == 0 || ebx == 0 {
        return None;
    }

    let ecx_hz = ecx as u64;
    let (crystal_hz, crystal_src) = if ecx_hz > 0 {
        (ecx_hz, CrystalSource::CpuidEcx)
    } else {
        // ECX = 0 : détecter la fréquence cristal depuis le modèle CPU.
        detect_crystal_from_model()
    };

    // TSC_Hz = crystal_hz × EBX / EAX
    let tsc_hz_128 = (crystal_hz as u128)
        .saturating_mul(ebx as u128)
        .checked_div(eax as u128)
        .unwrap_or(0);

    let tsc_hz = if tsc_hz_128 < TSC_MIN_HZ as u128 || tsc_hz_128 > TSC_MAX_HZ as u128 {
        0
    } else {
        tsc_hz_128 as u64
    };

    Some(CpuidLeaf15Result {
        denominator:  eax,
        numerator:    ebx,
        crystal_hz:   ecx,
        crystal_used: crystal_hz,
        tsc_hz,
        crystal_src,
    })
}

/// Lit CPUID leaf 0x16 et construit le résultat.
fn cpuid_leaf16_full() -> Option<CpuidLeaf16Result> {
    if max_cpuid_leaf() < 0x16 {
        return None;
    }

    let (eax, ebx, ecx) = read_cpuid_leaf_16();
    let base_mhz = eax & 0xFFFF;

    if base_mhz == 0 {
        return None;
    }

    let tsc_hz = (base_mhz as u64).saturating_mul(1_000_000);
    if tsc_hz < TSC_MIN_HZ || tsc_hz > TSC_MAX_HZ {
        return None;
    }

    Some(CpuidLeaf16Result {
        base_mhz,
        max_mhz:  (ebx & 0xFFFF),
        bus_mhz:  (ecx & 0xFFFF),
        tsc_hz,
    })
}

// ── Détection du cristal par modèle CPU ───────────────────────────────────────

/// Détecte la fréquence cristal depuis l'identifiant de modèle CPUID.
///
/// Algorithme :
///   1. Lire EAX de CPUID leaf 1 → Family/Model/Stepping
///   2. Identifier la famille de CPU
///   3. Retourner la fréquence cristal connue pour cette famille
///
/// RÈGLE CALIBRATION-CPUID-02 : fallback si ECX de leaf 0x15 est nul.
fn detect_crystal_from_model() -> (u64, CrystalSource) {
    let (family, model, _stepping) = read_cpu_family_model();

    // Famille Intel = 6 (tous les Core, Atom modernes).
    if family == 6 {
        return intel_crystal_by_model(model);
    }

    // AMD : family 0x17 (Zen), 0x19 (Zen 3), 0x1A (Zen 4)
    // AMD n'implémente généralement pas leaf 0x15 avec EBX, mais par précaution :
    if family >= 0x17 {
        // AMD Zen+ utilise 100 MHz reference → Crystal n'est pas pertinent ici
        // mais si leaf 0x15 est présente avec EBX/EAX valides, le cristal est ~100 MHz.
        return (CRYSTAL_FREQ_100MHZ, CrystalSource::Standard24Mhz); // notation approximative
    }

    // Fallback Intel générique : 24 MHz.
    (CRYSTAL_FREQ_24MHZ, CrystalSource::DefaultIntel24Mhz)
}

/// Identifie le cristal Intel par numéro de modèle (Family 6).
fn intel_crystal_by_model(model: u32) -> (u64, CrystalSource) {
    // Référence Intel Developer Manual Volume 3B, Table 18-2.
    match model {
        // Skylake-S/H/U (0x4E, 0x5E) — Kaby Lake (0x8E, 0x9E) — Coffee Lake (0x9E)
        // Whiskey Lake (0x8E) — Comet Lake (0xA5, 0xA6) — Rocket Lake (0xA7)
        0x4E | 0x5E | 0x8E | 0x9E | 0xA5 | 0xA6 | 0xA7 => {
            (CRYSTAL_FREQ_24MHZ, CrystalSource::Standard24Mhz)
        }
        // Ice Lake (0x7E, 0x7D)
        0x7E | 0x7D => {
            (CRYSTAL_FREQ_24MHZ, CrystalSource::Standard24Mhz)
        }
        // Tiger Lake (0x8C, 0x8D) — Alder Lake (0x97, 0x9A)
        // Raptor Lake (0xB7, 0xBA) — Meteor Lake (0xAA, 0xAC)
        0x8C | 0x8D | 0x97 | 0x9A | 0xB7 | 0xBA | 0xBF | 0xAA | 0xAC => {
            (CRYSTAL_FREQ_384MHZ, CrystalSource::Standard384Mhz)
        }
        // Atom Goldmont (0x5C) — Goldmont Plus (0x7A)
        0x5C | 0x7A => {
            (CRYSTAL_FREQ_192MHZ, CrystalSource::Standard192Mhz)
        }
        // Atom Tremont (0x86, Elkhart Lake 0x96, Jasper Lake 0x9C)
        0x86 | 0x96 | 0x9C => {
            (CRYSTAL_FREQ_192MHZ, CrystalSource::Standard192Mhz)
        }
        // Haswell (0x3C, 0x3F), Broadwell (0x47, 0x4F) — pas de crystal 0x15 valide
        // mais si EBX/EAX valides, on suppose 25 MHz (fréquence BCLK standard).
        0x3C | 0x3F | 0x47 | 0x4F => {
            // Avant Skylake, 0x15 est rarement valide — si on arrive ici, fallback.
            (CRYSTAL_FREQ_24MHZ, CrystalSource::DefaultIntel24Mhz)
        }
        // Tout autre modèle Intel Family 6 → 24 MHz par défaut (meilleur choix conservateur).
        _ => (CRYSTAL_FREQ_24MHZ, CrystalSource::DefaultIntel24Mhz),
    }
}

// ── Primitives CPUID bas niveau ───────────────────────────────────────────────

/// Retourne le max leaf CPUID standard supporté.
fn max_cpuid_leaf() -> u32 {
    let max: u32;
    // SAFETY: CPUID leaf 0, non-privilégié.
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0u32 => max,
            inout("ecx") 0u32 => _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    max
}

/// Lit CPUID leaf 0x15 en préservant rbx.
/// Retourne (EAX=denominator, EBX=numerator, ECX=crystal_hz).
fn read_cpuid_leaf_15() -> (u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    // SAFETY: CPUID non-privilégié. push rbx / pop rbx préserve le callee-saved rbx
    //         sans perturber le compilateur (stack frame manuelle, outside llvm).
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_r:e}, ebx",
            "pop rbx",
            inout("eax") 0x15u32 => eax_out,
            inout("ecx") 0u32    => ecx_out,
            out("edx") _,
            ebx_r = out(reg) ebx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out, ecx_out)
}

/// Lit CPUID leaf 0x16 en préservant rbx.
/// Retourne (EAX=base_mhz, EBX=max_mhz, ECX=bus_mhz).
fn read_cpuid_leaf_16() -> (u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_r:e}, ebx",
            "pop rbx",
            inout("eax") 0x16u32 => eax_out,
            inout("ecx") 0u32    => ecx_out,
            out("edx") _,
            ebx_r = out(reg) ebx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out, ecx_out)
}

/// Lit Family/Model/Stepping depuis CPUID leaf 1 EAX.
///
/// Format Intel EAX leaf 1 :
///   [3:0]   Stepping ID
///   [7:4]   Model
///   [11:8]  Family ID
///   [13:12] Processor Type
///   [19:16] Extended Model ID
///   [27:20] Extended Family ID
fn read_cpu_family_model() -> (u32, u32, u32) {
    let eax: u32;
    // SAFETY: CPUID leaf 1, non-privilégié.
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => eax,
            inout("ecx") 0u32 => _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    let stepping     = eax & 0xF;
    let base_model   = (eax >> 4)  & 0xF;
    let base_family  = (eax >> 8)  & 0xF;
    let ext_model    = (eax >> 16) & 0xF;
    let ext_family   = (eax >> 20) & 0xFF;

    // Intel/AMD : family réelle = base_family + ext_family (si base_family = 0xF ou 0x6)
    let family = if base_family == 0xF {
        base_family + ext_family
    } else {
        base_family
    };
    // Model réel = (ext_model << 4) | base_model  (si family == 6 ou 0xF)
    let model = if base_family == 6 || base_family == 0xF {
        (ext_model << 4) | base_model
    } else {
        base_model
    };

    (family, model, stepping)
}

// ── Utilitaires exportés ──────────────────────────────────────────────────────

/// Retourne la fréquence cristal détectée pour le CPU courant sans calculer TSC.
/// Utile pour le diagnostic système.
pub fn detect_crystal_hz() -> u64 {
    let (hz, _src) = detect_crystal_from_model();
    hz
}

/// Vérifie si le TSC nominal est cohérent avec la plage attendue pour la
/// fréquence de base CPU annoncée par leaf 0x16.
///
/// Retourne `true` si les deux sources sont cohérentes (écart < 5%).
pub fn cross_check_0x15_vs_0x16() -> bool {
    let hz15 = match cpuid_tsc_hz() { Some(v) => v, None => return true };
    let hz16 = match cpuid_tsc_hz_leaf16() { Some(v) => v, None => return true };
    if hz16 == 0 { return true; }
    let diff = if hz15 > hz16 { hz15 - hz16 } else { hz16 - hz15 };
    let pct_x100 = (diff as u128 * 10_000) / hz16 as u128;
    pct_x100 <= 500  // ≤ 5% d'écart acceptable
}

/// Vérifie la disponibilité complète du chemin CPUID nominal.
/// Retourne true si au moins leaf 0x15 ou 0x16 peut fournir une fréquence.
pub fn cpuid_nominal_available() -> bool {
    cpuid_tsc_hz().is_some() || cpuid_tsc_hz_leaf16().is_some()
}

/// Retourne un label lisible de la source CPUID disponible.
pub fn cpuid_source_label() -> &'static str {
    if cpuid_leaf15_available() {
        if let Some(r) = cpuid_leaf15_full() {
            if r.tsc_hz > 0 { return "CPUID-0x15"; }
        }
    }
    if cpuid_leaf16_available() {
        return "CPUID-0x16";
    }
    "CPUID-N/A"
}

/// Retourne la famille étendue du CPU (pour diagnostics).
pub fn cpu_extended_family() -> u32 {
    read_cpu_family_model().0
}

/// Retourne le modèle étendu du CPU (pour diagnostics).
pub fn cpu_extended_model() -> u32 {
    read_cpu_family_model().1
}
