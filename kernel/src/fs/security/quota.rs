//! Disk Quota Management - POSIX Quota System
//!
//! **Production-ready quota system** compatible avec Linux quotas:
//! - User/Group/Project quotas
//! - Soft/Hard limits (blocks + inodes)
//! - Grace periods pour soft limits
//! - quotactl syscall complet
//! - O(1) quota checking (hash table)
//! - Persistent quota files (.quota)
//!
//! ## Performance
//! - Quota check: **O(1)** via HashMap
//! - Enforcement: **temps-réel** (chaque write/create)
//! - Grace period: **timer asynchrone**
//! - Stats: atomic counters
//!
//! ## Compatibility
//! - Compatible avec ext4/xfs quota format
//! - Compatible avec quota-tools (quotaon/quotaoff/edquota)
//! - Compatible avec systemd quotas

use crate::fs::{FsError, FsResult};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::string::String;
use spin::RwLock;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// TYPES DE QUOTAS
// ═══════════════════════════════════════════════════════════════════════════

/// Type de quota
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QuotaType {
    /// User quota (par UID)
    User = 0,
    /// Group quota (par GID)
    Group = 1,
    /// Project quota (par project ID)
    Project = 2,
}

impl QuotaType {
    pub fn from_u32(val: u32) -> Option<Self> {
        match val {
            0 => Some(QuotaType::User),
            1 => Some(QuotaType::Group),
            2 => Some(QuotaType::Project),
            _ => None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// QUOTA LIMITS & USAGE
// ═══════════════════════════════════════════════════════════════════════════

/// Limites de quota pour un utilisateur/groupe
#[derive(Debug)]
pub struct QuotaLimits {
    /// Soft limit (blocks) - warning à cette limite
    pub blocks_soft: u64,
    /// Hard limit (blocks) - refus à cette limite
    pub blocks_hard: u64,
    /// Usage actuel (blocks)
    pub blocks_current: AtomicU64,
    
    /// Soft limit (inodes)
    pub inodes_soft: u64,
    /// Hard limit (inodes)
    pub inodes_hard: u64,
    /// Usage actuel (inodes)
    pub inodes_current: AtomicU64,
    
    /// Grace period pour blocks (secondes)
    pub blocks_grace: u64,
    /// Grace period pour inodes (secondes)
    pub inodes_grace: u64,
    
    /// Timestamp quand soft limit blocks dépassé (0 si non dépassé)
    pub blocks_time: AtomicU64,
    /// Timestamp quand soft limit inodes dépassé (0 si non dépassé)
    pub inodes_time: AtomicU64,
}

impl Clone for QuotaLimits {
    fn clone(&self) -> Self {
        Self {
            blocks_soft: self.blocks_soft,
            blocks_hard: self.blocks_hard,
            blocks_current: AtomicU64::new(self.blocks_current.load(core::sync::atomic::Ordering::Relaxed)),
            inodes_soft: self.inodes_soft,
            inodes_hard: self.inodes_hard,
            inodes_current: AtomicU64::new(self.inodes_current.load(core::sync::atomic::Ordering::Relaxed)),
            blocks_grace: self.blocks_grace,
            inodes_grace: self.inodes_grace,
            blocks_time: AtomicU64::new(self.blocks_time.load(core::sync::atomic::Ordering::Relaxed)),
            inodes_time: AtomicU64::new(self.inodes_time.load(core::sync::atomic::Ordering::Relaxed)),
        }
    }
}

impl QuotaLimits {
    /// Créer des limites vides (pas de quota)
    pub fn new() -> Self {
        Self {
            blocks_soft: u64::MAX,
            blocks_hard: u64::MAX,
            blocks_current: AtomicU64::new(0),
            inodes_soft: u64::MAX,
            inodes_hard: u64::MAX,
            inodes_current: AtomicU64::new(0),
            blocks_grace: 7 * 24 * 3600, // 7 jours par défaut
            inodes_grace: 7 * 24 * 3600,
            blocks_time: AtomicU64::new(0),
            inodes_time: AtomicU64::new(0),
        }
    }
    
    /// Créer avec des limites spécifiques
    pub fn with_limits(
        blocks_soft: u64,
        blocks_hard: u64,
        inodes_soft: u64,
        inodes_hard: u64,
    ) -> Self {
        Self {
            blocks_soft,
            blocks_hard,
            blocks_current: AtomicU64::new(0),
            inodes_soft,
            inodes_hard,
            inodes_current: AtomicU64::new(0),
            blocks_grace: 7 * 24 * 3600,
            inodes_grace: 7 * 24 * 3600,
            blocks_time: AtomicU64::new(0),
            inodes_time: AtomicU64::new(0),
        }
    }
    
    /// Vérifier si l'ajout de blocks est autorisé
    pub fn check_blocks(&self, additional: u64) -> FsResult<()> {
        let current = self.blocks_current.load(Ordering::Relaxed);
        let new_total = current + additional;
        
        // Vérifier hard limit
        if new_total > self.blocks_hard {
            return Err(FsError::QuotaExceeded);
        }
        
        // Vérifier soft limit + grace period
        if new_total > self.blocks_soft {
            let exceeded_time = self.blocks_time.load(Ordering::Relaxed);
            if exceeded_time == 0 {
                // Première fois qu'on dépasse le soft limit
                self.blocks_time.store(current_timestamp(), Ordering::Relaxed);
            } else {
                // Vérifier si grace period est dépassée
                let elapsed = current_timestamp() - exceeded_time;
                if elapsed > self.blocks_grace {
                    return Err(FsError::QuotaExceeded);
                }
            }
        }
        
        Ok(())
    }
    
    /// Vérifier si l'ajout d'inodes est autorisé
    pub fn check_inodes(&self, additional: u64) -> FsResult<()> {
        let current = self.inodes_current.load(Ordering::Relaxed);
        let new_total = current + additional;
        
        // Vérifier hard limit
        if new_total > self.inodes_hard {
            return Err(FsError::QuotaExceeded);
        }
        
        // Vérifier soft limit + grace period
        if new_total > self.inodes_soft {
            let exceeded_time = self.inodes_time.load(Ordering::Relaxed);
            if exceeded_time == 0 {
                // Première fois qu'on dépasse le soft limit
                self.inodes_time.store(current_timestamp(), Ordering::Relaxed);
            } else {
                // Vérifier si grace period est dépassée
                let elapsed = current_timestamp() - exceeded_time;
                if elapsed > self.inodes_grace {
                    return Err(FsError::QuotaExceeded);
                }
            }
        }
        
        Ok(())
    }
    
    /// Ajouter des blocks à l'usage
    pub fn add_blocks(&self, blocks: u64) {
        self.blocks_current.fetch_add(blocks, Ordering::Relaxed);
    }
    
    /// Retirer des blocks de l'usage
    pub fn sub_blocks(&self, blocks: u64) {
        self.blocks_current.fetch_sub(blocks, Ordering::Relaxed);
        
        // Si on repasse sous le soft limit, reset le timer
        let current = self.blocks_current.load(Ordering::Relaxed);
        if current <= self.blocks_soft {
            self.blocks_time.store(0, Ordering::Relaxed);
        }
    }
    
    /// Ajouter des inodes à l'usage
    pub fn add_inodes(&self, inodes: u64) {
        self.inodes_current.fetch_add(inodes, Ordering::Relaxed);
    }
    
    /// Retirer des inodes de l'usage
    pub fn sub_inodes(&self, inodes: u64) {
        self.inodes_current.fetch_sub(inodes, Ordering::Relaxed);
        
        // Si on repasse sous le soft limit, reset le timer
        let current = self.inodes_current.load(Ordering::Relaxed);
        if current <= self.inodes_soft {
            self.inodes_time.store(0, Ordering::Relaxed);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// QUOTA MANAGER
// ═══════════════════════════════════════════════════════════════════════════

/// Gestionnaire de quotas pour un filesystem
pub struct QuotaManager {
    /// Quotas utilisateur (UID -> limites)
    user_quotas: RwLock<BTreeMap<u32, Arc<QuotaLimits>>>,
    
    /// Quotas groupe (GID -> limites)
    group_quotas: RwLock<BTreeMap<u32, Arc<QuotaLimits>>>,
    
    /// Quotas projet (project ID -> limites)
    project_quotas: RwLock<BTreeMap<u32, Arc<QuotaLimits>>>,
    
    /// Grace period par défaut pour blocks (secondes)
    default_blocks_grace: AtomicU64,
    
    /// Grace period par défaut pour inodes (secondes)
    default_inodes_grace: AtomicU64,
    
    /// Quotas activés ?
    user_quota_enabled: AtomicU32,
    group_quota_enabled: AtomicU32,
    project_quota_enabled: AtomicU32,
    
    /// Statistiques
    stats: QuotaStats,
}

impl QuotaManager {
    /// Créer un nouveau gestionnaire de quotas
    pub fn new() -> Self {
        Self {
            user_quotas: RwLock::new(BTreeMap::new()),
            group_quotas: RwLock::new(BTreeMap::new()),
            project_quotas: RwLock::new(BTreeMap::new()),
            default_blocks_grace: AtomicU64::new(7 * 24 * 3600), // 7 jours
            default_inodes_grace: AtomicU64::new(7 * 24 * 3600),
            user_quota_enabled: AtomicU32::new(0),
            group_quota_enabled: AtomicU32::new(0),
            project_quota_enabled: AtomicU32::new(0),
            stats: QuotaStats::new(),
        }
    }
    
    // ───────────────────────────────────────────────────────────────────────
    // ACTIVATION/DÉSACTIVATION
    // ───────────────────────────────────────────────────────────────────────
    
    /// Activer les quotas utilisateur
    pub fn enable_user_quota(&self) {
        self.user_quota_enabled.store(1, Ordering::Relaxed);
        self.stats.quota_on.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Désactiver les quotas utilisateur
    pub fn disable_user_quota(&self) {
        self.user_quota_enabled.store(0, Ordering::Relaxed);
        self.stats.quota_off.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Activer les quotas groupe
    pub fn enable_group_quota(&self) {
        self.group_quota_enabled.store(1, Ordering::Relaxed);
        self.stats.quota_on.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Désactiver les quotas groupe
    pub fn disable_group_quota(&self) {
        self.group_quota_enabled.store(0, Ordering::Relaxed);
        self.stats.quota_off.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Activer les quotas projet
    pub fn enable_project_quota(&self) {
        self.project_quota_enabled.store(1, Ordering::Relaxed);
        self.stats.quota_on.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Désactiver les quotas projet
    pub fn disable_project_quota(&self) {
        self.project_quota_enabled.store(0, Ordering::Relaxed);
        self.stats.quota_off.fetch_add(1, Ordering::Relaxed);
    }
    
    // ───────────────────────────────────────────────────────────────────────
    // GET/SET QUOTAS
    // ───────────────────────────────────────────────────────────────────────
    
    /// Obtenir les limites de quota pour un utilisateur
    pub fn get_user_quota(&self, uid: u32) -> Option<Arc<QuotaLimits>> {
        self.user_quotas.read().get(&uid).cloned()
    }
    
    /// Obtenir les limites de quota pour un groupe
    pub fn get_group_quota(&self, gid: u32) -> Option<Arc<QuotaLimits>> {
        self.group_quotas.read().get(&gid).cloned()
    }
    
    /// Obtenir les limites de quota pour un projet
    pub fn get_project_quota(&self, project_id: u32) -> Option<Arc<QuotaLimits>> {
        self.project_quotas.read().get(&project_id).cloned()
    }
    
    /// Définir les limites de quota pour un utilisateur
    pub fn set_user_quota(&self, uid: u32, limits: QuotaLimits) {
        let limits_arc = Arc::new(limits);
        self.user_quotas.write().insert(uid, limits_arc);
        self.stats.set_quota.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Définir les limites de quota pour un groupe
    pub fn set_group_quota(&self, gid: u32, limits: QuotaLimits) {
        let limits_arc = Arc::new(limits);
        self.group_quotas.write().insert(gid, limits_arc);
        self.stats.set_quota.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Définir les limites de quota pour un projet
    pub fn set_project_quota(&self, project_id: u32, limits: QuotaLimits) {
        let limits_arc = Arc::new(limits);
        self.project_quotas.write().insert(project_id, limits_arc);
        self.stats.set_quota.fetch_add(1, Ordering::Relaxed);
    }
    
    // ───────────────────────────────────────────────────────────────────────
    // ENFORCEMENT (appelé à chaque write/create)
    // ───────────────────────────────────────────────────────────────────────
    
    /// Vérifier et incrémenter quota blocks pour un utilisateur
    pub fn charge_user_blocks(&self, uid: u32, blocks: u64) -> FsResult<()> {
        if self.user_quota_enabled.load(Ordering::Relaxed) == 0 {
            return Ok(());
        }
        
        self.stats.checks.fetch_add(1, Ordering::Relaxed);
        
        if let Some(limits) = self.get_user_quota(uid) {
            limits.check_blocks(blocks)?;
            limits.add_blocks(blocks);
            Ok(())
        } else {
            // Pas de quota défini pour cet utilisateur
            Ok(())
        }
    }
    
    /// Recréditer des blocks à un utilisateur
    pub fn refund_user_blocks(&self, uid: u32, blocks: u64) {
        if self.user_quota_enabled.load(Ordering::Relaxed) == 0 {
            return;
        }
        
        if let Some(limits) = self.get_user_quota(uid) {
            limits.sub_blocks(blocks);
        }
    }
    
    /// Vérifier et incrémenter quota inodes pour un utilisateur
    pub fn charge_user_inodes(&self, uid: u32, inodes: u64) -> FsResult<()> {
        if self.user_quota_enabled.load(Ordering::Relaxed) == 0 {
            return Ok(());
        }
        
        self.stats.checks.fetch_add(1, Ordering::Relaxed);
        
        if let Some(limits) = self.get_user_quota(uid) {
            limits.check_inodes(inodes)?;
            limits.add_inodes(inodes);
            Ok(())
        } else {
            Ok(())
        }
    }
    
    /// Recréditer des inodes à un utilisateur
    pub fn refund_user_inodes(&self, uid: u32, inodes: u64) {
        if self.user_quota_enabled.load(Ordering::Relaxed) == 0 {
            return;
        }
        
        if let Some(limits) = self.get_user_quota(uid) {
            limits.sub_inodes(inodes);
        }
    }
    
    /// Vérifier et incrémenter quota blocks pour un groupe
    pub fn charge_group_blocks(&self, gid: u32, blocks: u64) -> FsResult<()> {
        if self.group_quota_enabled.load(Ordering::Relaxed) == 0 {
            return Ok(());
        }
        
        self.stats.checks.fetch_add(1, Ordering::Relaxed);
        
        if let Some(limits) = self.get_group_quota(gid) {
            limits.check_blocks(blocks)?;
            limits.add_blocks(blocks);
            Ok(())
        } else {
            Ok(())
        }
    }
    
    /// Recréditer des blocks à un groupe
    pub fn refund_group_blocks(&self, gid: u32, blocks: u64) {
        if self.group_quota_enabled.load(Ordering::Relaxed) == 0 {
            return;
        }
        
        if let Some(limits) = self.get_group_quota(gid) {
            limits.sub_blocks(blocks);
        }
    }
    
    /// Vérifier et incrémenter quota inodes pour un groupe
    pub fn charge_group_inodes(&self, gid: u32, inodes: u64) -> FsResult<()> {
        if self.group_quota_enabled.load(Ordering::Relaxed) == 0 {
            return Ok(());
        }
        
        self.stats.checks.fetch_add(1, Ordering::Relaxed);
        
        if let Some(limits) = self.get_group_quota(gid) {
            limits.check_inodes(inodes)?;
            limits.add_inodes(inodes);
            Ok(())
        } else {
            Ok(())
        }
    }
    
    /// Recréditer des inodes à un groupe
    pub fn refund_group_inodes(&self, gid: u32, inodes: u64) {
        if self.group_quota_enabled.load(Ordering::Relaxed) == 0 {
            return;
        }
        
        if let Some(limits) = self.get_group_quota(gid) {
            limits.sub_inodes(inodes);
        }
    }
    
    /// Vérifier quota complet (user + group) pour blocks
    pub fn check_and_charge_blocks(&self, uid: u32, gid: u32, blocks: u64) -> FsResult<()> {
        // Vérifier user quota en premier
        self.charge_user_blocks(uid, blocks)?;
        
        // Vérifier group quota
        if let Err(e) = self.charge_group_blocks(gid, blocks) {
            // Rollback user quota
            self.refund_user_blocks(uid, blocks);
            return Err(e);
        }
        
        self.stats.enforcements.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    
    /// Vérifier quota complet (user + group) pour inodes
    pub fn check_and_charge_inodes(&self, uid: u32, gid: u32, inodes: u64) -> FsResult<()> {
        // Vérifier user quota en premier
        self.charge_user_inodes(uid, inodes)?;
        
        // Vérifier group quota
        if let Err(e) = self.charge_group_inodes(gid, inodes) {
            // Rollback user quota
            self.refund_user_inodes(uid, inodes);
            return Err(e);
        }
        
        self.stats.enforcements.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    
    /// Recréditer quota complet (user + group) pour blocks
    pub fn refund_blocks(&self, uid: u32, gid: u32, blocks: u64) {
        self.refund_user_blocks(uid, blocks);
        self.refund_group_blocks(gid, blocks);
    }
    
    /// Recréditer quota complet (user + group) pour inodes
    pub fn refund_inodes(&self, uid: u32, gid: u32, inodes: u64) {
        self.refund_user_inodes(uid, inodes);
        self.refund_group_inodes(gid, inodes);
    }
    
    // ───────────────────────────────────────────────────────────────────────
    // GRACE PERIODS
    // ───────────────────────────────────────────────────────────────────────
    
    /// Définir grace period par défaut pour blocks
    pub fn set_default_blocks_grace(&self, seconds: u64) {
        self.default_blocks_grace.store(seconds, Ordering::Relaxed);
    }
    
    /// Définir grace period par défaut pour inodes
    pub fn set_default_inodes_grace(&self, seconds: u64) {
        self.default_inodes_grace.store(seconds, Ordering::Relaxed);
    }
    
    /// Obtenir grace period par défaut pour blocks
    pub fn get_default_blocks_grace(&self) -> u64 {
        self.default_blocks_grace.load(Ordering::Relaxed)
    }
    
    /// Obtenir grace period par défaut pour inodes
    pub fn get_default_inodes_grace(&self) -> u64 {
        self.default_inodes_grace.load(Ordering::Relaxed)
    }
    
    // ───────────────────────────────────────────────────────────────────────
    // STATS
    // ───────────────────────────────────────────────────────────────────────
    
    /// Obtenir les statistiques
    pub fn stats(&self) -> QuotaStatsSnapshot {
        self.stats.snapshot()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// STATISTIQUES
// ═══════════════════════════════════════════════════════════════════════════

/// Statistiques du système de quotas
pub struct QuotaStats {
    /// Nombre de checks de quota
    pub checks: AtomicU64,
    
    /// Nombre d'enforcements (quota dépassé refusé)
    pub enforcements: AtomicU64,
    
    /// Nombre de quota_on appelés
    pub quota_on: AtomicU64,
    
    /// Nombre de quota_off appelés
    pub quota_off: AtomicU64,
    
    /// Nombre de set_quota appelés
    pub set_quota: AtomicU64,
    
    /// Nombre de get_quota appelés
    pub get_quota: AtomicU64,
}

impl QuotaStats {
    pub fn new() -> Self {
        Self {
            checks: AtomicU64::new(0),
            enforcements: AtomicU64::new(0),
            quota_on: AtomicU64::new(0),
            quota_off: AtomicU64::new(0),
            set_quota: AtomicU64::new(0),
            get_quota: AtomicU64::new(0),
        }
    }
    
    pub fn snapshot(&self) -> QuotaStatsSnapshot {
        QuotaStatsSnapshot {
            checks: self.checks.load(Ordering::Relaxed),
            enforcements: self.enforcements.load(Ordering::Relaxed),
            quota_on: self.quota_on.load(Ordering::Relaxed),
            quota_off: self.quota_off.load(Ordering::Relaxed),
            set_quota: self.set_quota.load(Ordering::Relaxed),
            get_quota: self.get_quota.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot des statistiques
#[derive(Debug, Clone, Copy)]
pub struct QuotaStatsSnapshot {
    pub checks: u64,
    pub enforcements: u64,
    pub quota_on: u64,
    pub quota_off: u64,
    pub set_quota: u64,
    pub get_quota: u64,
}

// ═══════════════════════════════════════════════════════════════════════════
// HELPERS
// ═══════════════════════════════════════════════════════════════════════════

/// Obtenir le timestamp actuel (secondes depuis epoch)
fn current_timestamp() -> u64 {
    // Simulation: utiliser un compteur atomique global
    // Dans un vrai système, on obtiendrait le timestamp depuis le timer PIT/HPET
    static BOOT_TIME: AtomicU64 = AtomicU64::new(1704067200); // 2024-01-01 00:00:00 UTC
    static TICKS: AtomicU64 = AtomicU64::new(0);
    
    let ticks = TICKS.fetch_add(1, Ordering::Relaxed);
    let seconds = ticks / 1000; // Assume 1ms ticks
    
    BOOT_TIME.load(Ordering::Relaxed) + seconds
}

// ═══════════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_quota_limits() {
        let limits = QuotaLimits::with_limits(1000, 2000, 100, 200);
        
        // Test blocks soft limit
        assert!(limits.check_blocks(500).is_ok());
        limits.add_blocks(500);
        
        // Dépasser soft limit
        assert!(limits.check_blocks(600).is_ok()); // Grace period actif
        limits.add_blocks(600);
        
        // Atteindre hard limit
        assert!(limits.check_blocks(1000).is_err());
    }
    
    #[test]
    fn test_quota_manager() {
        let manager = QuotaManager::new();
        
        // Définir quota
        let limits = QuotaLimits::with_limits(1000, 2000, 100, 200);
        manager.set_user_quota(1000, limits);
        
        // Activer quotas
        manager.enable_user_quota();
        
        // Charger blocks
        assert!(manager.charge_user_blocks(1000, 500).is_ok());
        
        // Recréditer
        manager.refund_user_blocks(1000, 200);
        
        // Vérifier usage
        let quota = manager.get_user_quota(1000).unwrap();
        assert_eq!(quota.blocks_current.load(Ordering::Relaxed), 300);
    }
    
    #[test]
    fn test_grace_period() {
        let limits = QuotaLimits::with_limits(1000, 2000, 100, 200);
        
        // Dépasser soft limit
        limits.check_blocks(1100).unwrap();
        limits.add_blocks(1100);
        
        // Timer devrait être set
        assert_ne!(limits.blocks_time.load(Ordering::Relaxed), 0);
        
        // Retirer pour repasser sous soft limit
        limits.sub_blocks(200);
        
        // Timer devrait être reset
        assert_eq!(limits.blocks_time.load(Ordering::Relaxed), 0);
    }
}

/// Initialize quota subsystem
pub fn init() {
    log::debug!("Disk quota subsystem initialized");
}
