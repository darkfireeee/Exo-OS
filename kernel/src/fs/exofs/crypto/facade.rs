//! High-level ExoFS crypto orchestrator, stats, health check, and tests.
//!
//! Extracted from `exofs/crypto/mod.rs` so `mod.rs` remains a pure wiring facade.

use core::sync::atomic::{AtomicU64, Ordering};

use super::{
    generate_salt, AuditKind, AuditSummary, CryptoShredder, KeyDerivation, KeyKind, KeySlotId,
    Nonce, OverwriteBlob, OverwriteStrategy, RotationSchedule, SecretReader, SecretWriter,
    ShredResult, XChaCha20Key, XChaCha20Poly1305, AUDIT_LOG, ENTROPY_POOL, KEY_STORAGE,
};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

/// Configuration du module de cryptographie.
#[derive(Debug, Clone)]
pub struct CryptoConfig {
    pub key_cache_size: usize,
    pub audit_enabled: bool,
    pub default_rotation: RotationSchedule,
    pub shred_strategy: OverwriteStrategy,
    pub max_stored_keys: usize,
    pub strict_nonce_check: bool,
}

impl Default for CryptoConfig {
    fn default() -> Self {
        Self {
            key_cache_size: 256,
            audit_enabled: true,
            default_rotation: RotationSchedule::AfterNUses(1_000_000),
            shred_strategy: OverwriteStrategy::DodThreePass,
            max_stored_keys: 4096,
            strict_nonce_check: true,
        }
    }
}

impl CryptoConfig {
    pub fn minimal() -> Self {
        Self {
            key_cache_size: 16,
            audit_enabled: false,
            default_rotation: RotationSchedule::OnDemand,
            shred_strategy: OverwriteStrategy::SinglePass,
            max_stored_keys: 64,
            strict_nonce_check: false,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.key_cache_size == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if self.max_stored_keys == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

/// Orchestrateur du module de cryptographie ExoFS.
pub struct CryptoModule {
    config: CryptoConfig,
    shredder: CryptoShredder,
}

impl CryptoModule {
    pub fn new(config: CryptoConfig) -> ExofsResult<Self> {
        config.validate()?;
        let shredder = CryptoShredder::new(config.shred_strategy);
        Ok(Self { config, shredder })
    }

    pub fn default_module() -> ExofsResult<Self> {
        Self::new(CryptoConfig::default())
    }

    pub fn encrypt_blob(
        &self,
        blob_id: u64,
        slot_id: KeySlotId,
        plaintext: &[u8],
    ) -> ExofsResult<alloc::vec::Vec<u8>> {
        let key_bytes = KEY_STORAGE.load_key_256(slot_id)?;
        let writer = SecretWriter::new(&key_bytes);
        let payload = writer.encrypt(plaintext)?;
        if self.config.audit_enabled {
            AUDIT_LOG.record(AuditKind::KeyWrapped, Some(slot_id), blob_id, true);
        }
        Ok(payload)
    }

    pub fn decrypt_blob(
        &self,
        blob_id: u64,
        slot_id: KeySlotId,
        payload: &[u8],
    ) -> ExofsResult<alloc::vec::Vec<u8>> {
        let key_bytes = KEY_STORAGE.load_key_256(slot_id)?;
        let reader = SecretReader::new(&key_bytes);
        let plain = reader.decrypt(payload).map_err(|e| {
            if self.config.audit_enabled {
                AUDIT_LOG.record(AuditKind::AuthFailure, Some(slot_id), blob_id, false);
            }
            e
        })?;
        if self.config.audit_enabled {
            AUDIT_LOG.record(AuditKind::KeyUnwrapped, Some(slot_id), blob_id, true);
        }
        Ok(plain)
    }

    pub fn shred_blob<O: OverwriteBlob>(
        &self,
        blob_id: u64,
        blob_size: u64,
        slot_id: Option<KeySlotId>,
        writer: &O,
    ) -> ExofsResult<ShredResult> {
        self.shredder
            .shred_blob(blob_id, blob_size, slot_id, Some(&KEY_STORAGE), writer)
    }

    pub fn config(&self) -> &CryptoConfig {
        &self.config
    }

    pub fn audit_summary(&self) -> ExofsResult<AuditSummary> {
        AUDIT_LOG.summary()
    }
}

pub fn encrypt_with_key(key: &[u8; 32], plaintext: &[u8]) -> ExofsResult<alloc::vec::Vec<u8>> {
    SecretWriter::new(key).encrypt(plaintext)
}

pub fn decrypt_with_key(key: &[u8; 32], payload: &[u8]) -> ExofsResult<alloc::vec::Vec<u8>> {
    SecretReader::new(key).decrypt(payload)
}

pub fn generate_key() -> ExofsResult<[u8; 32]> {
    generate_salt()
}

pub fn derive_key_simple(secret: &[u8], salt: &[u8; 32]) -> ExofsResult<[u8; 32]> {
    let dk = KeyDerivation::derive_key(secret, salt, b"data_encryption")?;
    Ok(*dk.as_bytes())
}

/// Compteurs atomiques des opérations cryptographiques.
pub struct CryptoStats {
    encrypt_ops: AtomicU64,
    decrypt_ops: AtomicU64,
    shred_ops: AtomicU64,
    derive_ops: AtomicU64,
    auth_errors: AtomicU64,
}

impl CryptoStats {
    const fn new() -> Self {
        Self {
            encrypt_ops: AtomicU64::new(0),
            decrypt_ops: AtomicU64::new(0),
            shred_ops: AtomicU64::new(0),
            derive_ops: AtomicU64::new(0),
            auth_errors: AtomicU64::new(0),
        }
    }

    pub fn record_encrypt(&self) {
        self.encrypt_ops.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_decrypt(&self) {
        self.decrypt_ops.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_shred(&self) {
        self.shred_ops.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_derive(&self) {
        self.derive_ops.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_auth_error(&self) {
        self.auth_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn encrypt_count(&self) -> u64 {
        self.encrypt_ops.load(Ordering::Relaxed)
    }
    pub fn decrypt_count(&self) -> u64 {
        self.decrypt_ops.load(Ordering::Relaxed)
    }
    pub fn shred_count(&self) -> u64 {
        self.shred_ops.load(Ordering::Relaxed)
    }
    pub fn derive_count(&self) -> u64 {
        self.derive_ops.load(Ordering::Relaxed)
    }
    pub fn auth_error_count(&self) -> u64 {
        self.auth_errors.load(Ordering::Relaxed)
    }

    pub fn total_cipher_ops(&self) -> u64 {
        self.encrypt_count().saturating_add(self.decrypt_count())
    }

    pub fn reset(&self) {
        self.encrypt_ops.store(0, Ordering::Relaxed);
        self.decrypt_ops.store(0, Ordering::Relaxed);
        self.shred_ops.store(0, Ordering::Relaxed);
        self.derive_ops.store(0, Ordering::Relaxed);
        self.auth_errors.store(0, Ordering::Relaxed);
    }
}

unsafe impl Sync for CryptoStats {}
unsafe impl Send for CryptoStats {}

pub static CRYPTO_STATS: CryptoStats = CryptoStats::new();

#[derive(Debug, Clone)]
pub struct HealthCheckResult {
    pub xchacha_ok: bool,
    pub entropy_ok: bool,
    pub kdf_ok: bool,
    pub storage_ok: bool,
    pub all_ok: bool,
}

impl HealthCheckResult {
    fn new(xchacha_ok: bool, entropy_ok: bool, kdf_ok: bool, storage_ok: bool) -> Self {
        let all_ok = xchacha_ok && entropy_ok && kdf_ok && storage_ok;
        Self {
            xchacha_ok,
            entropy_ok,
            kdf_ok,
            storage_ok,
            all_ok,
        }
    }
}

pub fn run_health_check() -> HealthCheckResult {
    let xchacha_ok = {
        let key = XChaCha20Key([0x42; 32]);
        let nonce = Nonce([0x24; 24]);
        let data = b"ExoFS crypto health check";
        let mut buf: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        if buf.try_reserve(data.len()).is_ok() {
            buf.extend_from_slice(data);
            if let Ok((ct, tag)) = XChaCha20Poly1305::encrypt(&key, &nonce, &[], data) {
                XChaCha20Poly1305::decrypt(&key, &nonce, &[], &ct, &tag)
                    .map(|pt| pt == data)
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            false
        }
    };

    let entropy_ok = {
        let b1 = ENTROPY_POOL.random_u64();
        let b2 = ENTROPY_POOL.random_u64();
        b1 != b2
    };

    let kdf_ok = {
        let salt = [0x55u8; 32];
        KeyDerivation::derive_key(b"health_check_secret", &salt, b"data_encryption")
            .map(|dk| dk.as_bytes().iter().any(|&b| b != 0))
            .unwrap_or(false)
    };

    let storage_ok = {
        let sid = KEY_STORAGE.store_key_256(&[0xEEu8; 32], KeyKind::Session);
        match sid {
            Ok(id) => {
                let load_ok = KEY_STORAGE
                    .load_key_256(id)
                    .map(|k| k == [0xEEu8; 32])
                    .unwrap_or(false);
                let _ = KEY_STORAGE.revoke_key(id);
                load_ok
            }
            Err(_) => false,
        }
    };

    HealthCheckResult::new(xchacha_ok, entropy_ok, kdf_ok, storage_ok)
}

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::{NullOverwriter, SECRET_HEADER_SIZE, SECRET_MAGIC};
    use super::*;

    fn key32() -> [u8; 32] {
        [0x99; 32]
    }

    #[test]
    fn test_encrypt_decrypt_convenience() {
        let k = key32();
        let p = encrypt_with_key(&k, b"integration test").test_unwrap();
        let d = decrypt_with_key(&k, &p).test_unwrap();
        assert_eq!(d, b"integration test");
    }

    #[test]
    fn test_generate_key_not_zero() {
        let k = generate_key().test_unwrap();
        assert_ne!(k, [0u8; 32]);
    }

    #[test]
    fn test_derive_key_simple() {
        let salt = [0xAA; 32];
        let k1 = derive_key_simple(b"password", &salt).test_unwrap();
        let k2 = derive_key_simple(b"password", &salt).test_unwrap();
        assert_eq!(k1, k2);
        let k3 = derive_key_simple(b"other", &salt).test_unwrap();
        assert_ne!(k1, k3);
    }

    #[test]
    fn test_crypto_config_default_valid() {
        assert!(CryptoConfig::default().validate().is_ok());
    }

    #[test]
    fn test_crypto_config_minimal_valid() {
        assert!(CryptoConfig::minimal().validate().is_ok());
    }

    #[test]
    fn test_crypto_config_invalid_cache_size() {
        let mut cfg = CryptoConfig::default();
        cfg.key_cache_size = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_crypto_config_invalid_max_keys() {
        let mut cfg = CryptoConfig::default();
        cfg.max_stored_keys = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_crypto_module_default() {
        assert!(CryptoModule::default_module().is_ok());
    }

    #[test]
    fn test_shred_blob_via_module() {
        let m = CryptoModule::default_module().test_unwrap();
        let nw = NullOverwriter;
        let res = m.shred_blob(100, 4096, None, &nw).test_unwrap();
        assert_eq!(res.blob_id, 100);
        assert!(res.physical_ok);
    }

    #[test]
    fn test_audit_summary_initially_zero() {
        let m = CryptoModule::default_module().test_unwrap();
        let _s = m.audit_summary();
    }

    #[test]
    fn test_secret_magic_constant() {
        assert_eq!(SECRET_MAGIC, [0xEF, 0x5E, 0x52, 0x44]);
    }

    #[test]
    fn test_secret_header_size_constant() {
        assert_eq!(SECRET_HEADER_SIZE, 52);
    }

    #[test]
    fn test_xchacha_key_zeroize() {
        let k = XChaCha20Key([0xAB; 32]);
        drop(k);
    }

    #[test]
    fn test_entropy_pool_accessible() {
        let _ = ENTROPY_POOL.random_32();
    }

    #[test]
    fn test_health_check_all_ok() {
        let hc = run_health_check();
        assert!(hc.xchacha_ok, "XChaCha20 failed");
        assert!(hc.entropy_ok, "Entropy failed");
        assert!(hc.kdf_ok, "KDF failed");
        assert!(hc.storage_ok, "Key storage failed");
        assert!(hc.all_ok, "Overall health check failed");
    }

    #[test]
    fn test_crypto_stats_increment() {
        CRYPTO_STATS.reset();
        CRYPTO_STATS.record_encrypt();
        CRYPTO_STATS.record_encrypt();
        CRYPTO_STATS.record_decrypt();
        assert_eq!(CRYPTO_STATS.encrypt_count(), 2);
        assert_eq!(CRYPTO_STATS.decrypt_count(), 1);
        assert_eq!(CRYPTO_STATS.total_cipher_ops(), 3);
    }

    #[test]
    fn test_crypto_stats_reset() {
        CRYPTO_STATS.record_shred();
        CRYPTO_STATS.reset();
        assert_eq!(CRYPTO_STATS.shred_count(), 0);
    }

    #[test]
    fn test_crypto_stats_auth_error() {
        CRYPTO_STATS.reset();
        CRYPTO_STATS.record_auth_error();
        assert_eq!(CRYPTO_STATS.auth_error_count(), 1);
    }

    #[test]
    fn test_derive_count() {
        CRYPTO_STATS.reset();
        CRYPTO_STATS.record_derive();
        CRYPTO_STATS.record_derive();
        assert_eq!(CRYPTO_STATS.derive_count(), 2);
    }
}
