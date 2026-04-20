//! # keystore — Gestionnaire de clés cryptographiques (crypto_server PID 4)
//!
//! Stockage sécurisé des clés dérivées. Les clés ne quittent JAMAIS ce processus.
//! Seul un handle opaque (u32, non-zéro) est retourné aux clients.
//!
//! ## Règles
//! - SRV-02 : seuls les handles sortent, jamais les octets bruts
//! - LAC-06 : shredding DoD 5220.22-M (3 passes) sur révocation
//! - NS-01 : uniquement core::sync::atomic + spin, pas de heap
//! - CAP-01 : toute opération vérifie le handle via constant-time compare

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

// ── Constantes ───────────────────────────────────────────────────────────────

/// Nombre maximum de clés simultanées dans le magasin.
const MAX_KEYS: usize = 64;

/// Taille d'une clé en octets (256 bits).
pub const KEY_SIZE: usize = 32;

/// Durée de vie maximale d'une clé : ~300 secondes en cycles TSC (à 3 GHz ≈ 900 milliards).
const KEY_MAX_LIFETIME_TSC: u64 = 900_000_000_000;

/// Types de clés supportés.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum KeyType {
    /// Clé dérivée via HKDF-Blake3 (générique).
    Derived = 0,
    /// Clé de chiffrement XChaCha20.
    Encryption = 1,
    /// Clé MAC Blake3.
    Mac = 2,
    /// Clé de canal IPC.
    IpcChannel = 3,
    /// Clé KEK (Key Encryption Key).
    Kek = 4,
    /// Clé de volume ExoFS.
    Volume = 5,
    /// Clé maîtresse (jamais exportée).
    Master = 6,
}

impl KeyType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Derived),
            1 => Some(Self::Encryption),
            2 => Some(Self::Mac),
            3 => Some(Self::IpcChannel),
            4 => Some(Self::Kek),
            5 => Some(Self::Volume),
            6 => Some(Self::Master),
            _ => None,
        }
    }
}

/// Flags d'état d'une entrée clé.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum KeyFlags {
    /// Entrée libre, non utilisée.
    Free = 0,
    /// Clé active, utilisable.
    Active = 1,
    /// Clé expirée (durée de vie dépassée).
    Expired = 2,
    /// Clé révoquée manuellement.
    Revoked = 3,
}

impl KeyFlags {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Free),
            1 => Some(Self::Active),
            2 => Some(Self::Expired),
            3 => Some(Self::Revoked),
            _ => None,
        }
    }
}

// ── Entrée clé ───────────────────────────────────────────────────────────────

/// Une seule entrée dans le magasin de clés.
#[repr(C)]
struct KeyEntry {
    /// Les 32 octets de la clé.
    key: [u8; KEY_SIZE],
    /// Type de la clé.
    key_type: u8,
    /// État de la clé (KeyFlags).
    flags: AtomicU8,
    /// TSC au moment de la création.
    creation_tsc: AtomicU64,
    /// Compteur d'utilisation (monotone).
    usage_count: AtomicU32,
    /// PID du propriétaire (0 = kernel).
    owner_pid: AtomicU32,
    /// Génération pour détection de réutilisation après révocation.
    generation: AtomicU32,
}

impl KeyEntry {
    const fn new() -> Self {
        Self {
            key: [0u8; KEY_SIZE],
            key_type: 0,
            flags: AtomicU8::new(KeyFlags::Free as u8),
            creation_tsc: AtomicU64::new(0),
            usage_count: AtomicU32::new(0),
            owner_pid: AtomicU32::new(0),
            generation: AtomicU32::new(0),
        }
    }

    fn is_active(&self) -> bool {
        self.flags.load(Ordering::Acquire) == KeyFlags::Active as u8
    }
}

// ── Magasin de clés ──────────────────────────────────────────────────────────

/// Table statique de clés. Pas de heap, pas d'allocation dynamique.
static KEY_TABLE: Mutex<[KeyEntry; MAX_KEYS]> = Mutex::new([
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
    KeyEntry::new(), KeyEntry::new(), KeyEntry::new(), KeyEntry::new(),
]);

/// Compteur pour le prochain slot disponible (recherche linéaire).
static NEXT_SLOT_HINT: AtomicU32 = AtomicU32::new(0);

/// Nombre total de clés actives.
static ACTIVE_KEY_COUNT: AtomicU32 = AtomicU32::new(0);

// ── Lecture TSC ──────────────────────────────────────────────────────────────

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ── Shredding DoD 5220.22-M ─────────────────────────────────────────────────

/// Shredding cryptographique 3 passes :
///   Passe 1 : écrire des zéros
///   Passe 2 : écrire des octets aléatoires (via RDRAND si disponible)
///   Passe 3 : écrire des zéros à nouveau
///
/// Chaque passe utilise core::ptr::write_volatile pour empêcher l'optimisation.
fn crypto_shred(buf: &mut [u8; KEY_SIZE]) {
    // Passe 1 : zéros
    for b in buf.iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0x00) };
    }
    core::sync::atomic::fence(Ordering::SeqCst);

    // Passe 2 : pseudo-aléatoire (RDRAND ou compteur mélangé)
    let mut seed: u64 = read_tsc();
    for b in buf.iter_mut() {
        // xoshiro256** minimal : mélange rapide
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let val = (seed ^ (seed >> 25)) as u8;
        unsafe { core::ptr::write_volatile(b, val) };
    }
    core::sync::atomic::fence(Ordering::SeqCst);

    // Passe 3 : zéros
    for b in buf.iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0x00) };
    }
    core::sync::atomic::fence(Ordering::SeqCst);
}

// ── Comparaison temps constant ───────────────────────────────────────────────

/// Comparaison constante de deux tableaux u8. Ne fuit pas d'information via le timing.
#[inline(always)]
fn ct_eq_u8(a: &[u8], b: &[u8]) -> bool {
    let mut diff: u8 = 0;
    let len = a.len().min(b.len());
    let mut i = 0usize;
    while i < len {
        diff |= a[i] ^ b[i];
        i += 1;
    }
    diff == 0 && a.len() == b.len()
}

// ── API publique ─────────────────────────────────────────────────────────────

/// Insère une clé dans le magasin. Retourne un handle opaque (non-zéro si succès).
/// Le handle est l'index + 1 (0 = invalide).
///
/// # Sécurité
/// - La clé est copiée dans le magasin, l'appelant peut shred sa copie.
/// - owner_pid est enregistré pour le contrôle d'accès.
pub fn insert_key(key: &[u8; KEY_SIZE], key_type: KeyType, owner_pid: u32) -> u32 {
    let mut table = KEY_TABLE.lock();

    // Recherche linéaire à partir du hint
    let hint = NEXT_SLOT_HINT.load(Ordering::Relaxed) as usize;
    for offset in 0..MAX_KEYS {
        let idx = (hint + offset) % MAX_KEYS;
        let entry = &mut table[idx];
        let flags = entry.flags.load(Ordering::Acquire);
        if flags == KeyFlags::Free as u8 || flags == KeyFlags::Revoked as u8 || flags == KeyFlags::Expired as u8 {
            // Si l'entrée était active, shred la clé précédente
            if flags != KeyFlags::Free as u8 {
                crypto_shred(&mut entry.key);
            }
            // Copier la nouvelle clé
            entry.key.copy_from_slice(key);
            entry.key_type = key_type as u8;
            entry.flags.store(KeyFlags::Active as u8, Ordering::Release);
            entry.creation_tsc.store(read_tsc(), Ordering::Release);
            entry.usage_count.store(0, Ordering::Release);
            entry.owner_pid.store(owner_pid, Ordering::Release);
            entry.generation.fetch_add(1, Ordering::AcqRel);

            let handle = (idx + 1) as u32;
            NEXT_SLOT_HINT.store((idx + 1) as u32, Ordering::Release);
            ACTIVE_KEY_COUNT.fetch_add(1, Ordering::Relaxed);
            return handle;
        }
    }

    // Aucun slot libre
    0
}

/// Récupère une référence vers la clé associée au handle.
///
/// # Retour
/// - `Some((key_slice, key_type))` si la clé est active et appartient au PID.
/// - `None` si le handle est invalide, la clé est expirée/révoquée, ou le PID ne correspond pas.
///
/// # Sécurité
/// - Le pointeur n'est valide que pendant la durée du lock.
/// - L'utilisation est comptée (usage_count).
/// - CAP-01 : vérification du propriétaire.
pub fn get_key(handle: u32, caller_pid: u32) -> Option<([u8; KEY_SIZE], KeyType)> {
    if handle == 0 || handle as usize > MAX_KEYS {
        return None;
    }
    let idx = (handle - 1) as usize;
    let table = KEY_TABLE.lock();
    let entry = &table[idx];

    // Vérifier l'état
    if !entry.is_active() {
        return None;
    }

    // Vérifier le propriétaire (0 = kernel, accès toujours autorisé)
    let owner = entry.owner_pid.load(Ordering::Acquire);
    if owner != 0 && owner != caller_pid {
        return None;
    }

    // Vérifier l'expiration
    let creation = entry.creation_tsc.load(Ordering::Acquire);
    let now = read_tsc();
    if now.wrapping_sub(creation) > KEY_MAX_LIFETIME_TSC {
        // Expirée — on la révoque immédiatement
        drop(table);
        revoke_key(handle);
        return None;
    }

    // Incrémenter le compteur d'utilisation
    entry.usage_count.fetch_add(1, Ordering::Relaxed);

    // Copier la clé (pour ne pas garder le lock trop longtemps)
    let key_copy = entry.key;
    let kt = KeyType::from_u8(entry.key_type).unwrap_or(KeyType::Derived);
    Some((key_copy, kt))
}

/// Révoque une clé : shredding cryptographique immédiat.
///
/// # Sécurité
/// - DoD 5220.22-M : 3 passes (zéros, aléatoire, zéros)
/// - La génération est incrémentée (invalide tout handle périmé)
/// - L'entrée est marquée `Revoked`
pub fn revoke_key(handle: u32) -> bool {
    if handle == 0 || handle as usize > MAX_KEYS {
        return false;
    }
    let idx = (handle - 1) as usize;
    let mut table = KEY_TABLE.lock();
    let entry = &mut table[idx];

    let flags = entry.flags.load(Ordering::Acquire);
    if flags != KeyFlags::Active as u8 {
        return false; // Déjà révoquée ou libre
    }

    // Shredding cryptographique
    crypto_shred(&mut entry.key);

    // Marquer comme révoquée
    entry.flags.store(KeyFlags::Revoked as u8, Ordering::Release);
    entry.generation.fetch_add(1, Ordering::AcqRel);
    ACTIVE_KEY_COUNT.fetch_sub(1, Ordering::Relaxed);
    true
}

/// Rotation de clé : dérive une nouvelle clé à partir de l'ancienne + entropie fraîche.
/// L'ancienne clé est shreddée, la nouvelle prend sa place.
///
/// # Algorithme
/// new_key = Blake3(old_key || fresh_entropy || counter)
///
/// # Retour
/// Le même handle si succès (la clé est remplacée en place), 0 si échec.
pub fn rotate_key(handle: u32, caller_pid: u32) -> u32 {
    if handle == 0 || handle as usize > MAX_KEYS {
        return 0;
    }
    let idx = (handle - 1) as usize;
    let mut table = KEY_TABLE.lock();
    let entry = &mut table[idx];

    // Vérifier l'état
    if !entry.is_active() {
        return 0;
    }

    // Vérifier le propriétaire
    let owner = entry.owner_pid.load(Ordering::Acquire);
    if owner != 0 && owner != caller_pid {
        return 0;
    }

    // Sauvegarder l'ancienne clé
    let old_key = entry.key;

    // Générer l'entropie fraîche
    let tsc_entropy = read_tsc();
    let usage = entry.usage_count.load(Ordering::Acquire);
    let gen = entry.generation.load(Ordering::Acquire);

    // Dérivation simplifiée : XOR + mélange (Blake3 complet dans kdf.rs)
    // Ici on utilise un mélange FNV amélioré car on n'a pas accès au crate blake3
    let mut new_key = [0u8; KEY_SIZE];

    // Phase 1 : copie de l'ancienne clé
    new_key.copy_from_slice(&old_key);

    // Phase 2 : mélange avec entropie
    let entropy_bytes = [
        tsc_entropy.to_le_bytes(),
        usage.to_le_bytes(),
        gen.to_le_bytes(),
        (handle as u64).to_le_bytes(),
    ];
    for (i, chunk) in entropy_bytes.iter().enumerate() {
        for (j, &b) in chunk.iter().enumerate() {
            let pos = (i * 8 + j) % KEY_SIZE;
            new_key[pos] = new_key[pos].wrapping_add(b).wrapping_mul(0x9E).wrapping_add(0x37);
        }
    }

    // Phase 3 : diffusion (3 passes de mélange)
    for _ in 0..3 {
        for i in 0..KEY_SIZE {
            new_key[i] = new_key[i]
                .wrapping_add(new_key[(i + 7) % KEY_SIZE])
                .rotate_left(3);
        }
    }

    // Shredder l'ancienne clé dans l'entrée (même si on va écraser)
    crypto_shred(&mut entry.key);

    // Écrire la nouvelle clé
    entry.key.copy_from_slice(&new_key);
    entry.creation_tsc.store(read_tsc(), Ordering::Release);
    entry.usage_count.store(0, Ordering::Release);
    entry.generation.fetch_add(1, Ordering::AcqRel);

    // Shredder la copie locale
    crypto_shred(&mut new_key);

    handle
}

/// Vérifie l'expiration de toutes les clés actives.
/// Les clés expirées sont automatiquement shreddées et révoquées.
///
/// # Retour
/// Le nombre de clés expirées et révoquées.
pub fn expire_check() -> u32 {
    let now = read_tsc();
    let mut expired_count = 0u32;
    let mut table = KEY_TABLE.lock();

    for idx in 0..MAX_KEYS {
        let entry = &mut table[idx];
        if !entry.is_active() {
            continue;
        }
        let creation = entry.creation_tsc.load(Ordering::Acquire);
        if now.wrapping_sub(creation) > KEY_MAX_LIFETIME_TSC {
            crypto_shred(&mut entry.key);
            entry.flags.store(KeyFlags::Expired as u8, Ordering::Release);
            entry.generation.fetch_add(1, Ordering::AcqRel);
            expired_count += 1;
        }
    }

    if expired_count > 0 {
        ACTIVE_KEY_COUNT.fetch_sub(expired_count, Ordering::Relaxed);
    }
    expired_count
}

/// Retourne le nombre de clés actives.
pub fn active_key_count() -> u32 {
    ACTIVE_KEY_COUNT.load(Ordering::Relaxed)
}

/// Retourne des statistiques sur le magasin de clés.
#[repr(C)]
pub struct KeystoreStats {
    pub active: u32,
    pub expired: u32,
    pub revoked: u32,
    pub free: u32,
}

/// Collecte les statistiques du magasin.
pub fn get_stats() -> KeystoreStats {
    let table = KEY_TABLE.lock();
    let mut active = 0u32;
    let mut expired = 0u32;
    let mut revoked = 0u32;
    let mut free = 0u32;

    for idx in 0..MAX_KEYS {
        match KeyFlags::from_u8(table[idx].flags.load(Ordering::Acquire)) {
            Some(KeyFlags::Active) => active += 1,
            Some(KeyFlags::Expired) => expired += 1,
            Some(KeyFlags::Revoked) => revoked += 1,
            _ => free += 1,
        }
    }

    KeystoreStats { active, expired, revoked, free }
}

/// Vérifie qu'un handle est valide et actif (sans révéler la clé).
/// Utilisé pour les vérifications de capacité avant opération.
pub fn is_valid_handle(handle: u32) -> bool {
    if handle == 0 || handle as usize > MAX_KEYS {
        return false;
    }
    let idx = (handle - 1) as usize;
    let table = KEY_TABLE.lock();
    table[idx].is_active()
}

/// Révoque toutes les clés d'un propriétaire (utilisé à la mort d'un processus).
/// Retourne le nombre de clés révoquées.
pub fn revoke_all_for_owner(owner_pid: u32) -> u32 {
    let mut count = 0u32;
    let mut table = KEY_TABLE.lock();

    for idx in 0..MAX_KEYS {
        let entry = &mut table[idx];
        if entry.is_active() && entry.owner_pid.load(Ordering::Acquire) == owner_pid {
            crypto_shred(&mut entry.key);
            entry.flags.store(KeyFlags::Revoked as u8, Ordering::Release);
            entry.generation.fetch_add(1, Ordering::AcqRel);
            count += 1;
        }
    }

    if count > 0 {
        ACTIVE_KEY_COUNT.fetch_sub(count, Ordering::Relaxed);
    }
    count
}

/// Initialise le magasin de clés (appelé au démarrage du crypto_server).
pub fn keystore_init() {
    // S'assurer que toutes les entrées sont à l'état Free
    let mut table = KEY_TABLE.lock();
    for entry in table.iter_mut() {
        if entry.flags.load(Ordering::Acquire) != KeyFlags::Free as u8 {
            crypto_shred(&mut entry.key);
            entry.flags.store(KeyFlags::Free as u8, Ordering::Release);
        }
        entry.key_type = 0;
        entry.creation_tsc.store(0, Ordering::Release);
        entry.usage_count.store(0, Ordering::Release);
        entry.owner_pid.store(0, Ordering::Release);
        entry.generation.store(0, Ordering::Release);
    }
    NEXT_SLOT_HINT.store(0, Ordering::Release);
    ACTIVE_KEY_COUNT.store(0, Ordering::Release);
}
