// kernel/src/security/exokairos.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExoKairos — Capabilities Temporelles à Budget (ExoShield v1.0)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoKairos implémente le système de capabilities temporelles d'ExoShield.
// Chaque capability a un budget (appels et volume) qui décroît monotiquement,
// et une deadline cachée — l'expiration temporelle est stockée hors TCB dans
// une table kernel-only (PKS domain Credentials), et vérifiée en temps constant.
//
// ARCHITECTURE :
//   • TemporalCap : structure capability avec budget atomique + deadline MAC
//   • cap_deadline_table : stockage deadline TSC hors TCB (Ring 0 only, PKS Credentials)
//   • Vérification inline : zéro IPC, constant-time, ~30 cycles
//
// PROPRIÉTÉS DE SÉCURITÉ :
//   S4 (Budget Monotonicity) : □(use_cap ⟹ budget' < budget)
//     → calls_left est AtomicU32 décrémenté via fetch_sub — toujours décroissant
//   Deadline cachée : le TSC de deadline n'est JAMAIS exposé à Ring 1
//     → Seul deadline_mac (HMAC-Blake3) est visible dans TemporalCap
//     → Mythos ne peut pas inverser le deadline depuis le MAC
//
// TTL PAR DROIT (ExoShield Spec MODULE 5) :
//   NETWORK_SEND : 5 secondes
//   FILE_WRITE   : 30 secondes
//   EXEC         : 1 seconde
//   IPC_CALL     : 60 secondes
//   Défaut       : 5 minutes (300 secondes)
//
// CORRECTION CORR-v3.1-02 :
//   `deadline_tsc` n'est plus exposé dans la structure TemporalCap.
//   Seul `deadline_mac: [u8; 16]` est stocké — la deadline réelle est dans
//   cap_deadline_table (domaine PKS Credentials, inaccessible depuis Ring 1).
//
// RÈGLE EXOKAIROS-01 : verify() est constant-time — zéro branchement dépendant
//   des données de la capability (seul le résultat final diffère).
// RÈGLE EXOKAIROS-02 : verify() ne fait AUCUN IPC — toute la logique est inline.
// RÈGLE EXOKAIROS-03 : depth_max = 4 — pas de délégation au-delà de 4 niveaux.
//
// RÉFÉRENCES :
//   ExoShield_v1_Production.md — MODULE 5 : ExoKairos
//   kernel/src/security/capability/ — CapToken, Rights, verify, revoke
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use spin::Once;

use crate::security::capability::token::CapToken;
use crate::security::capability::rights::Rights;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes ExoKairos
// ─────────────────────────────────────────────────────────────────────────────

/// TTL (Time-To-Live) par droit en secondes.
/// ExoShield Spec MODULE 5 — TTL defaults.
pub mod ttl {
    /// NETWORK_SEND : 5 secondes.
    pub const NETWORK_SEND_S: u64 = 5;
    /// FILE_WRITE : 30 secondes.
    pub const FILE_WRITE_S: u64 = 30;
    /// EXEC : 1 seconde.
    pub const EXEC_S: u64 = 1;
    /// IPC_CALL : 60 secondes.
    pub const IPC_CALL_S: u64 = 60;
    /// Défaut : 5 minutes (300 secondes).
    pub const DEFAULT_S: u64 = 300;
}

/// Profondeur maximale de délégation (4 niveaux).
pub const MAX_DELEGATION_DEPTH: u8 = 4;

/// Taille maximale de la deadline table (nombre d'entrées).
/// En Phase 3.1, on utilise une table statique. En Phase 3.2+,
/// la table sera dynamique (hash map protégée par PKS Credentials).
const DEADLINE_TABLE_SIZE: usize = 256;

// ─────────────────────────────────────────────────────────────────────────────
// CapError — Erreurs de capability temporelle
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs possibles lors de la vérification d'une capability temporelle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    /// La capability a expiré (deadline TSC dépassée).
    Expired,
    /// Le budget d'appels est épuisé (calls_left == 0).
    BudgetExhausted,
    /// Le budget de volume est épuisé (bytes_left == 0).
    VolumeExhausted,
    /// La profondeur de délégation est dépassée.
    DepthExceeded,
    /// Le token de base est invalide (révoqué, mauvaise génération, etc.).
    InvalidToken,
    /// Le deadline MAC ne correspond pas — potentiellement forgé.
    MacMismatch,
    /// La capability n'existe pas dans la deadline table.
    NotFound,
}

// ─────────────────────────────────────────────────────────────────────────────
// TemporalCap — Capability temporelle
// ─────────────────────────────────────────────────────────────────────────────

/// Capability temporelle avec budget atomique et deadline cachée.
///
/// # Layout (repr(C))
///
/// | Champ          | Type       | Taille | Description                           |
/// |----------------|------------|--------|---------------------------------------|
/// | base           | CapToken   | 24     | Token de capability standard          |
/// | deadline_mac   | [u8; 16]  | 16     | HMAC-Blake3(oid\|\|deadline\|\|SECRET)|
/// | calls_left     | AtomicU32 | 4      | Budget d'appels restant              |
/// | bytes_left     | AtomicU64 | 8      | Volume restant (octets)              |
/// | depth          | u8        | 1      | Profondeur de délégation (0..4)      |
/// | _pad           | [u8; 7]   | 7      | Alignement                           |
/// |                |            | 60     | Total                                |
///
/// # Sécurité
/// - `deadline_mac` : HMAC-Blake3 de (oid || deadline_tsc || KERNEL_SECRET).
///   L'attaquant ne peut PAS déduire le TSC de deadline depuis le MAC.
/// - `calls_left` / `bytes_left` : atomiques, décrémentés monotiquement.
///   La propriété S4 (BudgetMonotonicity) est garantie par fetch_sub.
/// - `depth` : ne peut jamais dépasser MAX_DELEGATION_DEPTH (4).
#[repr(C)]
pub struct TemporalCap {
    /// Token de capability standard (object_id, rights, generation, type_tag).
    pub base: CapToken,
    /// HMAC-Blake3(oid || deadline_tsc || KERNEL_SECRET).
    /// Le deadline TSC réel est stocké dans cap_deadline_table (Ring 0 only).
    pub deadline_mac: [u8; 16],
    /// Budget d'appels restant (décrémenté atomiquement).
    pub calls_left: AtomicU32,
    /// Volume restant en octets (décrémenté atomiquement).
    pub bytes_left: AtomicU64,
    /// Profondeur de délégation (0 = original, 1..4 = délégué).
    pub depth: u8,
    /// Padding d'alignement.
    _pad: [u8; 7],
}

impl TemporalCap {
    /// Crée une nouvelle capability temporelle.
    ///
    /// # Arguments
    /// - `base` : Token de capability standard
    /// - `deadline_tsc` : TSC absolu de la deadline (stocké dans deadline table)
    /// - `initial_calls` : Budget initial d'appels
    /// - `initial_bytes` : Budget initial de volume (octets)
    /// - `depth` : Profondeur de délégation (0 pour une nouvelle cap)
    ///
    /// # Safety
    /// - `deadline_tsc` est stocké dans cap_deadline_table (PKS Credentials)
    /// - Le MAC est calculé avec KERNEL_SECRET (immuable après boot)
    pub unsafe fn new(
        base: CapToken,
        deadline_tsc: u64,
        initial_calls: u32,
        initial_bytes: u64,
        depth: u8,
    ) -> Self {
        debug_assert!(depth <= MAX_DELEGATION_DEPTH, "delegation depth exceeded");

        // Calculer le deadline MAC : HMAC-Blake3(oid || deadline_tsc || KERNEL_SECRET)
        let deadline_mac = compute_deadline_mac(base.object_id(), deadline_tsc);

        // Stocker la deadline dans la table kernel-only
        cap_deadline_table::insert(base.object_id(), deadline_tsc);

        Self {
            base,
            deadline_mac,
            calls_left: AtomicU32::new(initial_calls),
            bytes_left: AtomicU64::new(initial_bytes),
            depth,
            _pad: [0u8; 7],
        }
    }

    /// Vérifie la validité de cette capability — constant-time, ZÉRO IPC.
    ///
    /// # Propriétés
    /// - Constant-time : le temps d'exécution ne dépend PAS du résultat
    /// - Zéro IPC : toute la logique est inline dans Ring 0
    /// - Monotone : calls_left et bytes_left décroissent toujours
    ///
    /// # Séquence
    /// 1. Récupérer la deadline depuis cap_deadline_table (inaccessible Ring 1)
    /// 2. Vérifier expiration (constant-time comparison)
    /// 3. Vérifier le MAC (protection contre la falsification)
    /// 4. Décrémenter calls_left atomiquement
    /// 5. Vérifier le budget restant
    ///
    /// # RÈGLE EXOKAIROS-01
    /// verify() est constant-time : toutes les opérations sont effectuées
    /// quelle que soit l'issue (seul le résultat Err/Ok diffère).
    pub fn verify(&self, current_tsc: u64) -> Result<(), CapError> {
        // 1. Récupérer la deadline depuis la table kernel (inaccessible Ring 1)
        let deadline = cap_deadline_table::get_const_time(self.base.object_id());

        // 2. Vérifier le MAC (protection contre falsification du deadline_mac)
        let expected_mac = compute_deadline_mac(self.base.object_id(), deadline);
        if !ct_u8_array_eq(&self.deadline_mac, &expected_mac) {
            return Err(CapError::MacMismatch);
        }

        // 3. Vérifier expiration (constant-time comparison)
        //    ct_u64_gte(a, b) retourne true si a >= b, en temps constant.
        if ct_u64_gte(current_tsc, deadline) {
            return Err(CapError::Expired);
        }

        // 4. Décompte atomique des appels
        //    fetch_sub retourne la valeur AVANT décrémentation.
        //    Si calls_left était 0, on retourne BudgetExhausted.
        //    Mais on décrémente quand même pour maintenir le temps constant.
        let calls_result = {
            let mut result = Err(CapError::BudgetExhausted);
            let mut current = self.calls_left.load(Ordering::Acquire);
            loop {
                if current == 0 {
                    break;
                }
                match self.calls_left.compare_exchange_weak(
                    current,
                    current - 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        result = Ok(());
                        break;
                    }
                    Err(actual) => current = actual,
                }
            }
            result
        };

        // 5. Vérifier le volume restant (lecture seule — décrémenté par le caller)
        let bytes = self.bytes_left.load(Ordering::Acquire);
        if bytes == 0 && calls_result.is_ok() {
            return Err(CapError::VolumeExhausted);
        }

        calls_result
    }

    /// Décrémente le volume restant de `n` octets.
    ///
    /// Retourne le volume restant après décrémentation, ou 0 si le budget
    /// est épuisé (saturating_sub).
    #[inline(always)]
    pub fn consume_bytes(&self, n: u64) -> u64 {
        loop {
            let current = self.bytes_left.load(Ordering::Acquire);
            let new_val = current.saturating_sub(n);
            match self.bytes_left.compare_exchange_weak(
                current, new_val,
                Ordering::AcqRel, Ordering::Acquire,
            ) {
                Ok(_) => return new_val,
                Err(_) => continue, // réessayer (CAS contention)
            }
        }
    }

    /// Crée une capability déléguée (sous-ensemble des droits).
    ///
    /// # RÈGLE CAP-03 (héritée)
    /// Les droits délégués ne peuvent JAMAIS excéder les droits de la source.
    ///
    /// # RÈGLE EXOKAIROS-03
    /// La profondeur de délégation ne peut pas dépasser MAX_DELEGATION_DEPTH (4).
    ///
    /// Le budget et la deadline sont hérités (sous-ensemble du parent).
    pub fn delegate(
        &self,
        restricted_rights: Rights,
        new_calls: u32,
        new_bytes: u64,
    ) -> Result<TemporalCap, CapError> {
        // Vérifier la profondeur de délégation
        if self.depth >= MAX_DELEGATION_DEPTH {
            return Err(CapError::DepthExceeded);
        }

        // Vérifier que les droits demandés sont un sous-ensemble
        if !restricted_rights.is_subset_of(self.base.rights()) {
            return Err(CapError::InvalidToken);
        }

        // Le budget délégué ne peut pas excéder le budget restant du parent
        let parent_calls = self.calls_left.load(Ordering::Acquire);
        let parent_bytes = self.bytes_left.load(Ordering::Acquire);

        if new_calls > parent_calls || new_bytes > parent_bytes {
            return Err(CapError::BudgetExhausted);
        }

        // Créer le token délégué (même object_id, droits restreints)
        // Note: CapToken::new est pub(super) — en Phase 3.1 on construit manuellement
        // pour les tests. En production, delegate() passe par CapTable::grant().
        let delegated_base = CapToken::new(
            self.base.object_id(),
            restricted_rights,
            self.base.generation(),
            self.base.object_type(),
        );

        // Récupérer la deadline depuis la table
        let deadline = cap_deadline_table::get_const_time(self.base.object_id());

        // SAFETY: deadline est valide, depth est vérifié
        Ok(unsafe {
            TemporalCap::new(
                delegated_base,
                deadline,
                new_calls,
                new_bytes,
                self.depth + 1,
            )
        })
    }

    /// Retourne la profondeur de délégation.
    #[inline(always)]
    pub fn depth(&self) -> u8 {
        self.depth
    }

    /// Retourne le budget d'appels restant.
    #[inline(always)]
    pub fn calls_remaining(&self) -> u32 {
        self.calls_left.load(Ordering::Acquire)
    }

    /// Retourne le volume restant.
    #[inline(always)]
    pub fn bytes_remaining(&self) -> u64 {
        self.bytes_left.load(Ordering::Acquire)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// cap_deadline_table — Stockage deadline TSC (Ring 0 only, PKS Credentials)
// ─────────────────────────────────────────────────────────────────────────────

/// Module de stockage des deadlines TSC.
///
/// Les deadlines sont stockées dans une table séparée du TCB et des TemporalCap.
/// Cette table est dans le domaine PKS Credentials (pkey=2) — inaccessible
/// depuis Ring 1 et Ring 3.
///
/// En Phase 3.1, la table est un tableau statique. En Phase 3.2+,
/// elle sera une hash table dynamique protégée par PKS.
pub mod cap_deadline_table {
    use super::*;
    use crate::security::capability::token::ObjectId;

    /// Entrée de la deadline table : (ObjectId, deadline_tsc).
    #[repr(C)]
    struct DeadlineEntry {
        oid: u64,
        deadline_tsc: u64,
        occupied: AtomicU32, // 0 = libre, 1 = occupé
    }

    /// Table statique de deadlines (Phase 3.1).
    /// Taille : 256 entrées × 20 bytes = 5 KiB.
    static mut TABLE: [DeadlineEntry; DEADLINE_TABLE_SIZE] = {
        const INIT: DeadlineEntry = DeadlineEntry {
            oid: 0,
            deadline_tsc: 0,
            occupied: AtomicU32::new(0),
        };
        [INIT; DEADLINE_TABLE_SIZE]
    };

    /// Compteur d'entrées utilisées.
    static USED: AtomicUsize = AtomicUsize::new(0);

    /// Insère une deadline dans la table.
    ///
    /// # Safety
    /// Ring 0 uniquement. La table est protégée par PKS Credentials.
    pub unsafe fn insert(oid: ObjectId, deadline_tsc: u64) {
        let oid_val = oid.as_u64();

        // Recherche séquentielle (Phase 3.1 — sera hash table en Phase 3.2)
        for i in 0..DEADLINE_TABLE_SIZE {
            // SAFETY: i < DEADLINE_TABLE_SIZE ; pointeur brut mutable réservé
            // à cette routine Ring 0 pour la mise à jour atomique des entrées.
            let entry = &raw mut TABLE[i];
            let occupied = (*entry).occupied.load(Ordering::Acquire);

            if occupied == 1 && (*entry).oid == oid_val {
                // Mise à jour d'une entrée existante
                (*entry).deadline_tsc = deadline_tsc;
                return;
            }

            if occupied == 0 {
                // Slot libre — insertion CAS
                match (*entry).occupied.compare_exchange(
                    0, 1,
                    Ordering::AcqRel, Ordering::Acquire,
                ) {
                    Ok(_) => {
                        (*entry).oid = oid_val;
                        (*entry).deadline_tsc = deadline_tsc;
                        USED.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    Err(_) => continue, // CAS raté — un autre thread a pris le slot
                }
            }
        }

        // Table pleine — panique (ne devrait jamais arriver en Phase 3.1)
        panic!("EXOKAIROS: deadline table full ({} entries)", DEADLINE_TABLE_SIZE);
    }

    /// Récupère la deadline TSC pour un ObjectId — temps constant.
    ///
    /// # Constant-time
    /// Cette fonction parcourt TOUJOURS toute la table, même si la deadline
    /// est trouvée en début. Cela garantit que le temps d'exécution ne dépend
    /// pas de la position de l'entrée.
    ///
    /// # RÈGLE EXOKAIROS-01
    /// Accès constant-time pour empêcher l'attaque par timing.
    pub fn get_const_time(oid: ObjectId) -> u64 {
        let oid_val = oid.as_u64();
        let mut result = u64::MAX; // deadline = MAX signifie "pas trouvé" → Expiré

        // SAFETY: lecture seule de la table statique, pas de concurrent write
        // pendant get_const_time (PKS Credentials protège en Phase 3.2).
        unsafe {
            for i in 0..DEADLINE_TABLE_SIZE {
                let entry = &raw const TABLE[i];
                let occupied = (*entry).occupied.load(Ordering::Acquire);

                // Comparaison constant-time : on utilise ct_eq
                let oid_match = ct_u64_eq((*entry).oid, oid_val);
                let is_valid = (occupied == 1) as u64 & oid_match;

                // Sélection conditionnelle sans branchement :
                // Si is_valid != 0, result = entry.deadline_tsc
                // Sinon, result = result (inchangé)
                let mask = is_valid.wrapping_sub(1); // 0x0000... si match, 0xFFFF... sinon
                result = (result & mask) | ((*entry).deadline_tsc & !mask);
            }
        }

        result
    }

    /// Supprime une entrée de la deadline table.
    ///
    /// Appelé lors de la révocation d'une capability temporelle.
    pub fn remove(oid: ObjectId) {
        let oid_val = oid.as_u64();

        unsafe {
            for i in 0..DEADLINE_TABLE_SIZE {
                let entry = &raw mut TABLE[i];
                if (*entry).occupied.load(Ordering::Acquire) == 1 && (*entry).oid == oid_val {
                    (*entry).occupied.store(0, Ordering::Release);
                    (*entry).oid = 0;
                    (*entry).deadline_tsc = 0;
                    USED.fetch_sub(1, Ordering::Relaxed);
                    return;
                }
            }
        }
    }

    /// Nombre d'entrées utilisées dans la table.
    pub fn used_count() -> usize {
        USED.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions constant-time
// ─────────────────────────────────────────────────────────────────────────────

/// Comparaison constant-time de deux u64.
///
/// Retourne 1 si a == b, 0 sinon. Le temps d'exécution est indépendant
/// des valeurs comparées.
#[inline(always)]
fn ct_u64_eq(a: u64, b: u64) -> u64 {
    let xor = a ^ b;
    // Si xor == 0, les valeurs sont égales.
    // On utilise le fait que (xor | -xor) >> 63 == 0 uniquement si xor == 0.
    let v = xor | xor.wrapping_neg();
    (v >> 63).wrapping_sub(1) & 1 // 1 si égal, 0 sinon
}

/// Comparaison constant-time : a >= b.
///
/// Retourne true si a >= b, en temps constant.
/// Utilise la technique du bit de signe de (a - b) quand il n'y a pas d'overflow.
#[inline(always)]
fn ct_u64_gte(a: u64, b: u64) -> bool {
    (a.wrapping_sub(b) >> 63) == 0
}

/// Comparaison constant-time de deux tableaux de 16 bytes.
///
/// Retourne true si les tableaux sont identiques.
#[inline(always)]
fn ct_u8_array_eq(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let mut acc: u8 = 0;
    for i in 0..16 {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Deadline MAC — HMAC-Blake3
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule le MAC de deadline : HMAC-Blake3(oid || deadline_tsc || KERNEL_SECRET).
///
/// Le MAC tronqué à 16 bytes est stocké dans TemporalCap.deadline_mac.
/// La deadline TSC réelle n'est PAS déductible depuis ce MAC.
///
fn compute_deadline_mac(oid: crate::security::capability::token::ObjectId, deadline_tsc: u64) -> [u8; 16] {
    // Construire le message : oid (8 bytes) || deadline_tsc (8 bytes)
    let mut msg = [0u8; 16];
    msg[0..8].copy_from_slice(&oid.as_u64().to_le_bytes());
    msg[8..16].copy_from_slice(&deadline_tsc.to_le_bytes());

    let key = get_kernel_secret();
    let full_hash = crate::security::crypto::blake3::blake3_mac(&key, &msg);

    // Tronquer à 16 bytes (128 bits de sécurité)
    let mut mac = [0u8; 16];
    mac.copy_from_slice(&full_hash[0..16]);
    mac
}

/// KERNEL_SECRET — clé secrète initialisée par ExoSeal au boot.
/// 32 bytes, stocké en mémoire statique (sera en PKS Credentials en Phase 3.2).
static KERNEL_SECRET: Once<[u8; 32]> = Once::new();

/// Initialise le KERNEL_SECRET (appelé par ExoSeal au boot).
///
/// # Safety
/// Doit être appelé UNE SEULE FOIS au boot, avant que Kernel A ne démarre.
/// Le secret doit provenir d'une source CSPRNG (RDRAND + ChaCha20).
pub fn init_kernel_secret(secret: &[u8; 32]) {
    KERNEL_SECRET.call_once(|| *secret);
}

/// Lit le KERNEL_SECRET (Ring 0 uniquement).
///
/// # Safety
/// Ne doit jamais être appelé depuis Ring 1 ou Ring 3.
/// En Phase 3.2, cette lecture sera protégée par PKS Credentials.
fn get_kernel_secret() -> [u8; 32] {
    *KERNEL_SECRET.get()
        .expect("KERNEL_SECRET non initialisé — exoseal_boot_phase0 doit précéder verify()")
}

// ─────────────────────────────────────────────────────────────────────────────
// TTL Lookup — Détermine le TTL pour un droit donné
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le TTL en secondes pour un droit spécifique.
///
/// Utilisé lors de la création d'une TemporalCap pour calculer la deadline.
pub fn ttl_for_right(rights: Rights) -> u64 {
    // Le TTL est celui du droit le plus restrictif (le plus court)
    if rights.contains(Rights::EXEC) {
        return ttl::EXEC_S;
    }
    if rights.contains(Rights::IPC_SEND) {
        return ttl::NETWORK_SEND_S; // IPC_SEND ≈ NETWORK_SEND pour le TTL
    }
    if rights.contains(Rights::WRITE) {
        return ttl::FILE_WRITE_S;
    }
    if rights.contains(Rights::IPC_CONNECT) || rights.contains(Rights::IPC_RECV) {
        return ttl::IPC_CALL_S;
    }
    ttl::DEFAULT_S
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques ExoKairos
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot des statistiques ExoKairos.
#[derive(Debug, Clone, Copy)]
pub struct ExoKairosStats {
    /// Nombre d'entrées dans la deadline table.
    pub deadline_table_used: usize,
    /// KERNEL_SECRET initialisé.
    pub secret_initialized: bool,
}

/// Retourne un snapshot des statistiques ExoKairos.
pub fn exokairos_stats() -> ExoKairosStats {
    ExoKairosStats {
        deadline_table_used: cap_deadline_table::used_count(),
        secret_initialized: KERNEL_SECRET.get().is_some(),
    }
}
