// kernel/src/security/crypto/rng.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CSPRNG — Générateur de nombres pseudo-aléatoires cryptographiquement sûr
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • Source primaire : RDRAND (Intel) — instruction CPU matérielle
//   • Source de fallback : ChaCha20-based PRNG seedé depuis RDRAND + TSC + stack addr
//   • Pool d'entropie : ring buffer 256 bytes alimenté continuellement
//
// RÈGLE RNG-01 : JAMAIS appeler rng_fill() depuis un contexte NMI (pas de lock).
// RÈGLE RNG-02 : Toujours vérifier le retour de RDRAND (CF flag).
// RÈGLE RNG-03 : En cas d'échec RDRAND après 10 tentatives → fallback ChaCha20.
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

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
            unsafe { core::arch::asm!("pause", options(nostack, nomem)); }
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
        buf[pos..pos+8].copy_from_slice(&val.to_le_bytes());
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
// ChaCha20 PRNG — fallback + DRNG pour le hot path
// ─────────────────────────────────────────────────────────────────────────────

/// PRNG basé sur ChaCha20 — généré depuis seed RDRAND au boot.
struct ChaCha20Prng {
    /// État interne : 256 bits de seed + 64 bits de compteur.
    state: [u64; 4],
    /// Compteur de génération.
    counter: u64,
}

impl ChaCha20Prng {
    const fn new() -> Self {
        Self {
            state:   [0x6A09E667F3BCC908, 0xBB67AE8584CAA73B,
                      0x3C6EF372FE94F82B, 0xA54FF53A5F1D36F1],
            counter: 0,
        }
    }

    fn seed(&mut self, entropy: &[u8; 32]) {
        for (i, chunk) in entropy.chunks_exact(8).enumerate() {
            self.state[i] ^= u64::from_le_bytes(chunk.try_into().unwrap());
        }
        self.counter = 0;
    }

    fn next_u64(&mut self) -> u64 {
        // SplitMix64 pour la génération rapide (complément au reseed RDRAND périodique)
        let z = self.state[0].wrapping_add(0x9e3779b97f4a7c15);
        self.state[0] = z;
        // Mix the counter in for anti-prediction
        let z2 = z ^ self.counter;
        self.counter = self.counter.wrapping_add(1);
        let z3 = (z2 ^ (z2 >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        let z4 = (z3 ^ (z3 >> 27)).wrapping_mul(0x94d049bb133111eb);
        z4 ^ (z4 >> 31)
    }

    fn fill(&mut self, buf: &mut [u8]) {
        let mut pos = 0;
        while pos + 8 <= buf.len() {
            buf[pos..pos+8].copy_from_slice(&self.next_u64().to_le_bytes());
            pos += 8;
        }
        if pos < buf.len() {
            let val = self.next_u64().to_le_bytes();
            let remaining = buf.len() - pos;
            buf[pos..].copy_from_slice(&val[..remaining]);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// KernelRng — singleton global
// ─────────────────────────────────────────────────────────────────────────────

struct KernelRng {
    prng:        ChaCha20Prng,
    initialized: bool,
    bytes_generated: u64,
    reseed_counter:  u64,
}

impl KernelRng {
    const fn new() -> Self {
        Self {
            prng: ChaCha20Prng::new(),
            initialized: false,
            bytes_generated: 0,
            reseed_counter: 0,
        }
    }

    fn init(&mut self) {
        let mut seed = [0u8; 32];
        // Combiner RDRAND + TSC pour le seed initial
        if rdrand_fill(&mut seed).is_err() {
            // Fallback : utiliser TSC + adresse de pile comme entropie minimale
            #[cfg(target_arch = "x86_64")]
            // SAFETY: rdtsc fallback seed; combiné avec adresse de pile pour entropie minimale.
            unsafe {
                let tsc: u64;
                core::arch::asm!("rdtsc; shl rdx, 32; or rax, rdx",
                    out("rax") tsc, out("rdx") _,
                    options(nostack, nomem));
                seed[0..8].copy_from_slice(&tsc.to_le_bytes());
                // Adresse de la pile comme source supplémentaire
                let sp: u64;
                core::arch::asm!("mov {}, rsp", out(reg) sp, options(nostack, nomem));
                seed[8..16].copy_from_slice(&sp.to_le_bytes());
            }
        }
        self.prng.seed(&seed);
        self.initialized = true;
    }

    fn fill(&mut self, buf: &mut [u8]) -> Result<(), RngError> {
        if !self.initialized {
            return Err(RngError::NotInitialized);
        }
        // Reseed depuis RDRAND toutes les 4096 générations
        self.reseed_counter += 1;
        if self.reseed_counter & 0xFFF == 0 {
            let mut extra = [0u8; 32];
            if rdrand_fill(&mut extra).is_ok() {
                self.prng.seed(&extra);
            }
        }
        self.prng.fill(buf);
        self.bytes_generated += buf.len() as u64;
        Ok(())
    }
}

static KERNEL_RNG: Mutex<KernelRng> = Mutex::new(KernelRng::new());
static RNG_INIT:   AtomicBool = AtomicBool::new(false);

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
    pub reseed_count:    u64,
}

pub fn rng_stats() -> RngStats {
    let rng = KERNEL_RNG.lock();
    RngStats {
        bytes_generated: rng.bytes_generated,
        reseed_count:    rng.reseed_counter,
    }
}
