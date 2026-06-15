// kernel/src/security/crypto/rng.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CSPRNG — Générateur de nombres pseudo-aléatoires cryptographiquement sûr
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • Source primaire : RDRAND (Intel) — instruction CPU matérielle
//   • Générateur : ChaCha20 block function (RFC 8439) importée de
//     xchacha20_poly1305.rs — arithmétique u32 pure, zéro SIMD/SSE2.
//   • Reseed : toutes les 4096 blocs ChaCha20 générés, mixage RDRAND.
//   • Fallback : TSC + adresse de pile quand RDRAND échoue.
//
// RÈGLE CRYPTO-CRATES : ChaCha20 est la SEULE primitive crypto maison
// autorisée (la crate chacha20 déclenche LLVM ERROR: split 128-bit sur
// x86_64-unknown-none sans SSE2). L'implémentation est conforme RFC 8439.
//
// RÈGLE RNG-01 : JAMAIS appeler rng_fill() depuis un contexte NMI (pas de lock).
// RÈGLE RNG-02 : Toujours vérifier le retour de RDRAND (CF flag).
// RÈGLE RNG-03 : En cas d'échec RDRAND après 10 tentatives → fallback TSC+stack.
// ═══════════════════════════════════════════════════════════════════════════════

use super::xchacha20_poly1305::chacha20_block;
use crate::arch::x86_64::cpu::features::cpu_features_or_none;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// Nombre de blocs ChaCha20 avant un reseed obligatoire.
const RESEED_INTERVAL_BLOCKS: u64 = 4096;

/// Taille d'un bloc ChaCha20 en octets.
const CHACHA20_BLOCK_SIZE: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// RdrandError
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum RngError {
    /// RDRAND a échoué après le nombre maximum de tentatives.
    RdrandExhausted,
    /// RNG non initialisé.
    NotInitialized,
    /// Entropie insuffisante au démarrage.
    InsufficientEntropy,
}

// ─────────────────────────────────────────────────────────────────────────────
// RDRAND — lecture matérielle
// ─────────────────────────────────────────────────────────────────────────────

/// Tente de lire une valeur 64 bits depuis RDRAND.
/// Retourne Ok(value) ou Err si CF=0 (retentatives épuisées).
#[inline]
fn rdrand64() -> Result<u64, RngError> {
    #[cfg(target_arch = "x86_64")]
    {
        // Évite #UD si l'instruction RDRAND n'est pas supportée par le CPU/VM.
        if !cpu_features_or_none().map_or(false, |features| features.has_rdrand()) {
            return Err(RngError::RdrandExhausted);
        }

        let mut val: u64 = 0;
        let mut success: u8;
        for _ in 0..10 {
            // SAFETY: rdrand peut échouer légitimement (CF=0); val = 0 si échec, retry jusqu'à 10.
            unsafe {
                core::arch::asm!(
                    "rdrand {val}",
                    "setc {ok}",
                    val = out(reg) val,
                    ok  = out(reg_byte) success,
                    options(nostack, nomem),
                );
            }
            if success != 0 {
                return Ok(val);
            }
            // Pause entre les tentatives (x86 hint)
            // SAFETY: PAUSE est une hint d'attente pour le CPU — aucun effet de bord.
            unsafe {
                core::arch::asm!("pause", options(nostack, nomem));
            }
        }
        Err(RngError::RdrandExhausted)
    }
    #[cfg(not(target_arch = "x86_64"))]
    Err(RngError::RdrandExhausted)
}

/// Tente de lire une valeur 64 bits depuis RDSEED.
///
/// RDSEED expose la source d'entropie MATÉRIELLE brute (vs RDRAND = CSPRNG
/// matériel reseedé), donc une qualité d'entropie supérieure pour le seeding.
/// Retourne `Err` si non supporté ou CF=0 après 10 tentatives.
#[inline]
fn rdseed64() -> Result<u64, RngError> {
    #[cfg(target_arch = "x86_64")]
    {
        if !cpu_features_or_none().map_or(false, |features| features.has_rdseed()) {
            return Err(RngError::RdrandExhausted);
        }
        let mut val: u64 = 0;
        let mut success: u8;
        for _ in 0..10 {
            // SAFETY: rdseed peut échouer légitimement (CF=0) ; val=0 si échec, retry.
            unsafe {
                core::arch::asm!(
                    "rdseed {val}",
                    "setc {ok}",
                    val = out(reg) val,
                    ok  = out(reg_byte) success,
                    options(nostack, nomem),
                );
            }
            if success != 0 {
                return Ok(val);
            }
            // SAFETY: PAUSE = hint CPU, aucun effet de bord.
            unsafe {
                core::arch::asm!("pause", options(nostack, nomem));
            }
        }
        Err(RngError::RdrandExhausted)
    }
    #[cfg(not(target_arch = "x86_64"))]
    Err(RngError::RdrandExhausted)
}

/// Lit n bytes depuis RDRAND dans un buffer.
pub fn rdrand_fill(buf: &mut [u8]) -> Result<(), RngError> {
    let mut pos = 0;
    while pos + 8 <= buf.len() {
        let val = rdrand64()?;
        buf[pos..pos + 8].copy_from_slice(&val.to_le_bytes());
        pos += 8;
    }
    if pos < buf.len() {
        let val = rdrand64()?;
        let bytes = val.to_le_bytes();
        let remaining = buf.len() - pos;
        buf[pos..].copy_from_slice(&bytes[..remaining]);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers — TSC et stack pointer (fallback entropie)
// ─────────────────────────────────────────────────────────────────────────────

/// Lit le TSC (Time Stamp Counter).
#[inline]
fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let tsc: u64;
        // SAFETY: RDTSC est non-privilégiée — aucun effet de bord.
        unsafe {
            core::arch::asm!("rdtsc; shl rdx, 32; or rax, rdx",
                out("rax") tsc, out("rdx") _,
                options(nostack, nomem));
        }
        tsc
    }
    #[cfg(not(target_arch = "x86_64"))]
    0
}

/// Lit le pointeur de pile courant (source d'entropie supplémentaire).
#[inline]
fn read_sp() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let sp: u64;
        // SAFETY: lecture seule de RSP — aucun effet de bord.
        unsafe {
            core::arch::asm!("mov {}, rsp", out(reg) sp, options(nostack, nomem));
        }
        sp
    }
    #[cfg(not(target_arch = "x86_64"))]
    0
}

/// Rassemble de l'entropie de **toutes** les sources disponibles dans un pool,
/// puis la **conditionne par Blake3** → seed 32 octets. Retourne `true` si au
/// moins une source MATÉRIELLE (RDSEED/RDRAND) a contribué.
///
/// Robustesse (RÈGLE RNG-03 durcie) : l'ancien seed fallback dérivait UNIQUEMENT
/// de TSC+SP par un mélange ad-hoc (prédictible, non-whitené). Ici :
///   1. RDSEED (entropie matérielle vraie) ×4, puis RDRAND ×6 ;
///   2. **jitter TSC** — lectures répétées entrecoupées de PAUSE ; les bits de
///      poids faible varient (jitter d'horloge) = vraie source d'aléa temporel ;
///   3. pointeur de pile (entropie d'adressage / KASLR) ;
///   4. **conditionnement Blake3** du pool entier → 32 octets whitened.
/// Même sans RDRAND, le seed est conditionné cryptographiquement et agrège
/// plusieurs sources (plancher d'entropie relevé vs TSC brut).
fn gather_seed(out: &mut [u8; 32]) -> bool {
    fn push(pool: &mut [u8; 160], off: &mut usize, v: u64) {
        if *off + 8 <= pool.len() {
            pool[*off..*off + 8].copy_from_slice(&v.to_le_bytes());
            *off += 8;
        }
    }

    let mut pool = [0u8; 160];
    let mut off = 0usize;
    let mut hw = false;

    // 1. RDSEED — entropie matérielle « vraie ».
    for _ in 0..4 {
        if let Ok(v) = rdseed64() {
            push(&mut pool, &mut off, v);
            hw = true;
        }
    }
    // 2. RDRAND — CSPRNG matériel.
    for _ in 0..6 {
        if let Ok(v) = rdrand64() {
            push(&mut pool, &mut off, v);
            hw = true;
        }
    }
    // 3. Jitter TSC (aléa temporel) + 4. pointeur de pile.
    for _ in 0..8 {
        push(&mut pool, &mut off, read_tsc());
        // SAFETY: PAUSE = hint CPU, aucun effet de bord ; espace les lectures TSC.
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("pause", options(nostack, nomem));
        }
    }
    push(&mut pool, &mut off, read_sp());

    // 5. Conditionnement Blake3 → 32 octets whitened.
    let conditioned = super::blake3::blake3_hash(&pool[..off]);
    out.copy_from_slice(&conditioned);

    // Effacer le pool (peut contenir de l'entropie résiduelle sensible).
    for b in pool.iter_mut() {
        // SAFETY: write_volatile empêche l'élision de l'effacement.
        unsafe {
            core::ptr::write_volatile(b, 0);
        }
    }
    core::sync::atomic::fence(Ordering::SeqCst);

    hw
}

// ─────────────────────────────────────────────────────────────────────────────
// ChaCha20 CSPRNG — Générateur basé sur ChaCha20 block function
// ─────────────────────────────────────────────────────────────────────────────

/// CSPRNG basé sur la ChaCha20 block function (RFC 8439).
///
/// Génère un keystream ChaCha20 à partir d'une clé 256-bit et d'un nonce
/// 96-bit + compteur. Chaque appel à `chacha20_block` produit 64 octets
/// de données aléatoire cryptographiquement sûre.
struct ChaCha20Csprng {
    /// Clé ChaCha20 (256 bits).
    key: [u8; 32],
    /// Nonce ChaCha20 (96 bits).
    nonce: [u8; 12],
    /// Compteur de bloc ChaCha20 courant.
    counter: u32,
    /// Tampon du dernier bloc ChaCha20 généré (64 octets).
    buffer: [u8; CHACHA20_BLOCK_SIZE],
    /// Position de consommation dans le tampon.
    buffer_pos: usize,
    /// Nombre total de blocs générés depuis le dernier reseed.
    blocks_since_reseed: u64,
}

impl ChaCha20Csprng {
    const fn new() -> Self {
        Self {
            key: [0u8; 32],
            nonce: [0u8; 12],
            counter: 0,
            buffer: [0u8; CHACHA20_BLOCK_SIZE],
            buffer_pos: CHACHA20_BLOCK_SIZE, // Force la génération au premier appel
            blocks_since_reseed: 0,
        }
    }

    /// Initialise le CSPRNG avec un seed de 32 octets.
    fn seed(&mut self, entropy: &[u8; 32]) {
        self.key.copy_from_slice(entropy);
        // Dérive un nonce à partir du seed en utilisant les 12 derniers octets XOR
        for i in 0..12 {
            self.nonce[i] = entropy[i]
                .wrapping_add(entropy[i + 20])
                .wrapping_add(entropy[(i + 7) % 32]);
        }
        self.counter = 0;
        self.buffer_pos = CHACHA20_BLOCK_SIZE; // Force la régénération
        self.blocks_since_reseed = 0;
    }

    /// Reseed le CSPRNG avec de nouvelles données d'entropie.
    ///
    /// XOR la nouvelle entropie dans la clé et avance le nonce pour
    /// garantir un keystream différent après le reseed.
    fn reseed(&mut self, extra: &[u8; 32]) {
        for i in 0..32 {
            self.key[i] ^= extra[i];
        }
        // Avance le nonce pour garantir un keystream nouveau
        self.nonce[11] = self.nonce[11].wrapping_add(1);
        if self.nonce[11] == 0 {
            self.nonce[10] = self.nonce[10].wrapping_add(1);
        }
        self.counter = 0;
        self.buffer_pos = CHACHA20_BLOCK_SIZE; // Force la régénération
        self.blocks_since_reseed = 0;
    }

    /// Génère un nouveau bloc ChaCha20 et remplit le tampon.
    #[inline]
    fn refill_buffer(&mut self) {
        self.buffer = chacha20_block(&self.key, &self.nonce, self.counter);
        self.counter = self.counter.wrapping_add(1);
        self.buffer_pos = 0;
        self.blocks_since_reseed += 1;
    }

    /// Remplit un buffer avec des octets aléatoires depuis le keystream ChaCha20.
    fn fill(&mut self, buf: &mut [u8]) {
        let mut pos = 0;
        while pos < buf.len() {
            if self.buffer_pos >= CHACHA20_BLOCK_SIZE {
                self.refill_buffer();
            }
            let remaining = buf.len() - pos;
            let available = CHACHA20_BLOCK_SIZE - self.buffer_pos;
            let to_copy = remaining.min(available);
            buf[pos..pos + to_copy]
                .copy_from_slice(&self.buffer[self.buffer_pos..self.buffer_pos + to_copy]);
            self.buffer_pos += to_copy;
            pos += to_copy;
        }
    }

    /// Retourne le nombre de blocs générés depuis le dernier reseed.
    #[inline]
    fn blocks_since_reseed(&self) -> u64 {
        self.blocks_since_reseed
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// KernelRng — singleton global
// ─────────────────────────────────────────────────────────────────────────────

struct KernelRng {
    prng: ChaCha20Csprng,
    initialized: bool,
    /// Vrai si le seed (initial ou reseed) a reçu de l'entropie MATÉRIELLE
    /// (RDSEED/RDRAND). Faux = seed conditionné mais d'entropie dégradée.
    hw_seeded: bool,
    bytes_generated: u64,
    reseed_count: u64,
}

impl KernelRng {
    const fn new() -> Self {
        Self {
            prng: ChaCha20Csprng::new(),
            initialized: false,
            hw_seeded: false,
            bytes_generated: 0,
            reseed_count: 0,
        }
    }

    fn init(&mut self) {
        let mut seed = [0u8; 32];
        // Seed = pool multi-sources (RDSEED+RDRAND+jitter TSC+SP) conditionné Blake3.
        self.hw_seeded = gather_seed(&mut seed);
        self.prng.seed(&seed);
        // Zéroïser le seed sur la pile (write_volatile pour empêcher l'élision).
        for b in seed.iter_mut() {
            // SAFETY: effacement borné d'un buffer local.
            unsafe {
                core::ptr::write_volatile(b, 0);
            }
        }
        core::sync::atomic::fence(Ordering::SeqCst);
        self.initialized = true;
    }

    fn fill(&mut self, buf: &mut [u8]) -> Result<(), RngError> {
        if !self.initialized {
            return Err(RngError::NotInitialized);
        }

        // Reseed toutes les RESEED_INTERVAL_BLOCKS blocs — même pool multi-sources
        // conditionné Blake3 (jamais de fallback faible non-whitené).
        if self.prng.blocks_since_reseed() >= RESEED_INTERVAL_BLOCKS {
            let mut extra = [0u8; 32];
            let hw = gather_seed(&mut extra);
            self.prng.reseed(&extra);
            self.hw_seeded = self.hw_seeded || hw;
            for b in extra.iter_mut() {
                // SAFETY: effacement borné d'un buffer local.
                unsafe {
                    core::ptr::write_volatile(b, 0);
                }
            }
            core::sync::atomic::fence(Ordering::SeqCst);
            self.reseed_count += 1;
        }

        self.prng.fill(buf);
        self.bytes_generated += buf.len() as u64;
        Ok(())
    }
}

static KERNEL_RNG: Mutex<KernelRng> = Mutex::new(KernelRng::new());
static RNG_INIT: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le générateur de nombres aléatoires kernel.
/// Doit être appelé après l'initialisation CPU (RDRAND disponible).
pub fn rng_init() {
    if RNG_INIT.swap(true, Ordering::SeqCst) {
        return; // Déjà initialisé
    }
    KERNEL_RNG.lock().init();
}

/// Remplit un buffer avec des bytes cryptographiquement aléatoires.
pub fn rng_fill(buf: &mut [u8]) -> Result<(), RngError> {
    KERNEL_RNG.lock().fill(buf)
}

/// Génère un u64 aléatoire.
pub fn rng_u64() -> Result<u64, RngError> {
    let mut buf = [0u8; 8];
    rng_fill(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

/// Génère un u32 aléatoire.
pub fn rng_u32() -> Result<u32, RngError> {
    let mut buf = [0u8; 4];
    rng_fill(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

/// Génère un tableau de 32 bytes aléatoires (clé cryptographique).
pub fn rng_key32() -> Result<[u8; 32], RngError> {
    let mut key = [0u8; 32];
    rng_fill(&mut key)?;
    Ok(key)
}

/// Génère un nonce de 24 bytes (XChaCha20).
pub fn rng_nonce24() -> Result<[u8; 24], RngError> {
    let mut nonce = [0u8; 24];
    rng_fill(&mut nonce)?;
    Ok(nonce)
}

/// Retourne vrai si le RNG est initialisé et opérationnel.
#[inline(always)]
pub fn rng_is_ready() -> bool {
    RNG_INIT.load(Ordering::Acquire) && KERNEL_RNG.lock().initialized
}

/// Retourne vrai si le RNG a été seedé avec de l'entropie MATÉRIELLE
/// (RDSEED/RDRAND). Faux = seed conditionné Blake3 mais sans source matérielle
/// (entropie dégradée — un appelant générant une clé long-terme critique peut
/// choisir de différer/alerter sur cette base).
#[inline(always)]
pub fn rng_hw_seeded() -> bool {
    KERNEL_RNG.lock().hw_seeded
}

/// Statistiques RNG.
#[derive(Debug, Clone, Copy)]
pub struct RngStats {
    pub bytes_generated: u64,
    pub reseed_count: u64,
    /// Entropie matérielle obtenue au seeding (RDSEED/RDRAND).
    pub hw_seeded: bool,
}

pub fn rng_stats() -> RngStats {
    let rng = KERNEL_RNG.lock();
    RngStats {
        bytes_generated: rng.bytes_generated,
        reseed_count: rng.reseed_count,
        hw_seeded: rng.hw_seeded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le seed est conditionné Blake3 (jamais tout-zéro) et varie entre deux
    /// appels (jitter TSC + sources matérielles) — pas de seed faible constant.
    #[test]
    fn gather_seed_is_conditioned_and_varies() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        let _ = gather_seed(&mut a);
        let _ = gather_seed(&mut b);
        assert_ne!(a, [0u8; 32], "seed conditionné ne doit pas être tout-zéro");
        assert_ne!(a, b, "deux seeds successifs doivent différer (TSC/HW)");
    }

    /// Après init, le CSPRNG produit un flux non-nul qui varie d'un tirage à l'autre.
    #[test]
    fn rng_fill_is_nonzero_and_varies() {
        rng_init();
        let mut x = [0u8; 32];
        let mut y = [0u8; 32];
        rng_fill(&mut x).expect("rng ready");
        rng_fill(&mut y).expect("rng ready");
        assert_ne!(x, [0u8; 32]);
        assert_ne!(x, y);
    }
}
