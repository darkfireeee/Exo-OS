//! Integrity - Data integrity layer
//!
//! ## Modules
//! - `checksum`: Blake3 checksumming
//! - `journal`: Write-Ahead Logging
//! - `recovery`: Crash recovery
//! - `scrubbing`: Background verification
//! - `healing`: Auto-healing with error correction
//! - `validator`: Validation hooks
//!
//! ## Features
//! - End-to-end data integrity
//! - Crash consistency
//! - Automatic corruption detection and repair
//! - Background scrubbing

pub mod checksum;
pub mod journal;
pub mod recovery;
pub mod scrubbing;
pub mod healing;
pub mod validator;

use alloc::sync::Arc;
use crate::fs::FsResult;

/// Initialize integrity subsystem
pub fn init(journal_max_size: usize) {
    log::info!("Initializing integrity subsystem");

    // Initialize checksum manager
    checksum::init();

    // Initialize journal
    journal::init(journal_max_size);

    // Initialize recovery manager (depends on journal)
    let journal_ref = journal::global_journal();
    recovery::init(Arc::clone(journal_ref));

    // Initialize scrubber (depends on checksum)
    let checksum_mgr = Arc::new(checksum::ChecksumManager::new());
    scrubbing::init(Arc::clone(&checksum_mgr));

    // Initialize healer (depends on checksum)
    healing::init(Arc::clone(&checksum_mgr));

    // Initialize validators
    validator::init();

    log::info!("✓ Integrity subsystem initialized");
}

/// Get integrity statistics
pub fn get_stats() -> IntegrityStats {
    IntegrityStats {
        checksums_computed: checksum::global_checksum_manager().stats().computed.load(core::sync::atomic::Ordering::Relaxed),
        checksums_verified: checksum::global_checksum_manager().stats().verified.load(core::sync::atomic::Ordering::Relaxed),
        checksum_mismatches: checksum::global_checksum_manager().stats().mismatches.load(core::sync::atomic::Ordering::Relaxed),
        transactions_committed: journal::global_journal().stats().transactions_committed.load(core::sync::atomic::Ordering::Relaxed),
        blocks_scrubbed: scrubbing::global_scrubber().stats().blocks_scrubbed.load(core::sync::atomic::Ordering::Relaxed),
        corruptions_detected: healing::global_healer().stats().corruptions_detected.load(core::sync::atomic::Ordering::Relaxed),
        corruptions_repaired: healing::global_healer().stats().corruptions_repaired.load(core::sync::atomic::Ordering::Relaxed),
        validations_performed: validator::global_validators().stats().validations_performed.load(core::sync::atomic::Ordering::Relaxed),
    }
}

#[derive(Debug, Clone)]
pub struct IntegrityStats {
    pub checksums_computed: u64,
    pub checksums_verified: u64,
    pub checksum_mismatches: u64,
    pub transactions_committed: u64,
    pub blocks_scrubbed: u64,
    pub corruptions_detected: u64,
    pub corruptions_repaired: u64,
    pub validations_performed: u64,
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn test_integrity_init() {
        // Initialize integrity subsystem
        init(1000);

        // Verify all components initialized
        let stats = get_stats();
        assert_eq!(stats.transactions_committed, 0);
        assert_eq!(stats.checksums_computed, 0);
    }

    #[test]
    fn test_checksum_to_journal_integration() {
        init(1000);

        // Create data
        let data = b"Integration test data";

        // Compute checksum
        let checksum_mgr = checksum::global_checksum_manager();
        let hash = checksum_mgr.compute(data);

        // Create journal transaction with checksummed data
        let journal_ref = journal::global_journal();
        let tx = journal_ref.begin_transaction();

        let entry = journal::JournalEntry::with_data(
            tx.id(),
            journal::JournalOpType::Write,
            123, // inode
            456, // block
            data.to_vec(),
        );

        tx.add_entry(entry).expect("Failed to add entry");
        journal_ref.commit(&tx).expect("Failed to commit");

        // Verify both systems updated
        let stats = get_stats();
        assert!(stats.checksums_computed > 0);
        assert!(stats.transactions_committed > 0);
    }

    #[test]
    fn test_healing_with_checksums() {
        init(1000);

        let checksum_mgr = Arc::new(checksum::ChecksumManager::new());
        let healer = healing::Healer::new(Arc::clone(&checksum_mgr));

        // Create test data
        let data = b"Data to be encoded and potentially recovered";

        // Encode with Reed-Solomon
        let shards = healer.encode(data).expect("Encoding failed");
        assert_eq!(shards.len(), healing::RS_TOTAL_SHARDS);

        // Verify all checksums
        for shard in &shards {
            assert!(checksum_mgr.verify(&shard.data, &shard.checksum));
        }

        // Simulate corruption and recovery
        let valid_shards: Vec<&healing::Shard> = shards.iter().take(healing::RS_DATA_SHARDS).collect();
        let recovered = healer.repair(&valid_shards).expect("Recovery failed");

        // Verify recovered data
        assert_eq!(&recovered[..data.len()], data);
    }

    #[test]
    fn test_validator_integration() {
        init(1000);

        let validators = validator::global_validators();
        let ctx = validator::ValidationContext::new(0, 123, "write");

        let data = b"Test validation data";

        // Pre-validation
        let result = validators.validate_pre(&ctx, data);
        assert_eq!(result, validator::ValidationResult::Allow);

        // Post-validation
        let result = validators.validate_post(&ctx, data);
        assert_eq!(result, validator::ValidationResult::Allow);

        // Check stats
        let stats = get_stats();
        assert!(stats.validations_performed > 0);
    }

    #[test]
    fn test_full_integrity_workflow() {
        // Initialize all systems
        init(1000);

        let checksum_mgr = checksum::global_checksum_manager();
        let journal_ref = journal::global_journal();
        let healer = healing::global_healer();
        let validators = validator::global_validators();

        // 1. Create data
        let original_data = b"Full workflow integration test data";

        // 2. Validate before write
        let ctx = validator::ValidationContext::new(0, 1000, "write");
        assert_eq!(
            validators.validate_pre(&ctx, original_data),
            validator::ValidationResult::Allow
        );

        // 3. Compute checksum
        let hash = checksum_mgr.compute(original_data);
        assert_ne!(hash, checksum::Blake3Hash::zero());

        // 4. Create journal transaction
        let tx = journal_ref.begin_transaction();
        let entry = journal::JournalEntry::with_data(
            tx.id(),
            journal::JournalOpType::Write,
            1000,
            2000,
            original_data.to_vec(),
        );
        tx.add_entry(entry).expect("Failed to add entry");

        // 5. Commit transaction
        journal_ref.commit(&tx).expect("Failed to commit");

        // 6. Encode with Reed-Solomon for redundancy
        let shards = healer.encode(original_data).expect("Encoding failed");
        assert_eq!(shards.len(), healing::RS_TOTAL_SHARDS);

        // 7. Validate after write
        assert_eq!(
            validators.validate_post(&ctx, original_data),
            validator::ValidationResult::Allow
        );

        // 8. Verify all stats updated
        let stats = get_stats();
        assert!(stats.checksums_computed > 0);
        assert!(stats.transactions_committed > 0);
        assert!(stats.validations_performed > 0);

        log::info!("Full integrity workflow completed successfully");
        log::info!("Stats: {:?}", stats);
    }

    #[test]
    fn test_scrubbing_integration() {
        init(1000);

        let scrubber = scrubbing::global_scrubber();

        // Schedule a scrub request
        let request = scrubbing::ScrubRequest::new(0, 0, 10);
        scrubber.schedule(request);

        // Run scrubber
        if let Some(result) = scrubber.run() {
            assert_eq!(result.blocks_scrubbed, 10);
            log::info!("Scrubbed {} blocks, found {} errors",
                      result.blocks_scrubbed, result.errors_found.len());
        }
    }

    #[test]
    fn test_recovery_integration() {
        init(1000);

        let journal_ref = journal::global_journal();
        let recovery_mgr = recovery::global_recovery();

        // Create some transactions
        for i in 0..5 {
            let tx = journal_ref.begin_transaction();
            let entry = journal::JournalEntry::new(
                tx.id(),
                journal::JournalOpType::Write,
                i,
            );
            tx.add_entry(entry).expect("Failed to add entry");
            journal_ref.commit(&tx).expect("Failed to commit");
        }

        // Run recovery
        let report = recovery_mgr.recover().expect("Recovery failed");

        log::info!("Recovery completed: {} transactions replayed, {} errors found, {} errors fixed",
                  report.transactions_replayed,
                  report.errors_found.len(),
                  report.errors_fixed);
    }
}
