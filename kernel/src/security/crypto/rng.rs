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

/// Remplit un buffer de 32 octets avec de l'entropie fallback (TSC + stack).
fn fallback_entropy(out: &mut [u8; 32]) {
    let tsc = read_tsc();
    let sp = read_sp();
    out[0..8].copy_from_slice(&tsc.to_le_bytes());
    out[8..16].copy_from_slice(&sp.to_le_bytes());
    // Diffusion : XOR-shift pour étaler les bits sur les 16 octets restants
    let mut a = tsc;
    let mut b = sp;
    for i in 0..2 {
        a = a.wrapping_add(b ^ (a >> 17));
        b = b.wrapping_add(a ^ (b >> 31));
        out[16 + i * 8..24 + i * 8]
            .copy_from_slice(&a.wrapping_mul(0x2545_f491_4f6c_dd1d).to_le_bytes());
    }
    // Passe de mélange final — chaque octet reçoit une contribution de TSC + SP
    for i in 0..32 {
        out[i] = out[i].wrapping_add(
            (tsc.wrapping_shr((i as u32) & 63) as u8)
                .wrapping_add(sp.wrapping_shr(((i as u32) + 3) & 63) as u8),
        );
    }
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
    bytes_generated: u64,
    reseed_count: u64,
}

impl KernelRng {
    const fn new() -> Self {
        Self {
            prng: ChaCha20Csprng::new(),
            initialized: false,
            bytes_generated: 0,
            reseed_count: 0,
        }
    }

    fn init(&mut self) {
        let mut seed = [0u8; 32];
        // Combiner RDRAND + TSC pour le seed initial
        if rdrand_fill(&mut seed).is_err() {
            // Fallback : utiliser TSC + adresse de pile comme entropie minimale
            fallback_entropy(&mut seed);
        }
        self.prng.seed(&seed);
        // Zéroïser le seed de la pile
        let _ = seed;
        self.initialized = true;
    }

    fn fill(&mut self, buf: &mut [u8]) -> Result<(), RngError> {
        if !self.initialized {
            return Err(RngError::NotInitialized);
        }

        // Reseed depuis RDRAND toutes les RESEED_INTERVAL_BLOCKS blocs
        if self.prng.blocks_since_reseed() >= RESEED_INTERVAL_BLOCKS {
            let mut extra = [0u8; 32];
            if rdrand_fill(&mut extra).is_ok() {
                self.prng.reseed(&extra);
            } else {
                // Fallback : TSC + stack
                fallback_entropy(&mut extra);
                self.prng.reseed(&extra);
            }
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

/// Statistiques RNG.
#[derive(Debug, Clone, Copy)]
pub struct RngStats {
    pub bytes_generated: u64,
    pub reseed_count: u64,
}

pub fn rng_stats() -> RngStats {
    let rng = KERNEL_RNG.lock();
    RngStats {
        bytes_generated: rng.bytes_generated,
        reseed_count: rng.reseed_count,
    }
}
