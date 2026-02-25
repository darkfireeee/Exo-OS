// drivers/fs/src/ext4/journal.rs — Vérification état journal JBD2  (exo-os-driver-fs)
//
// RÈGLE FS-EXT4-03 : JAMAIS rejouer le journal Linux depuis Exo-OS.
// Lire l'état uniquement pour déterminer si readable (s_start == 0 → clean).

pub const JBD2_MAGIC: u32 = 0xC03B3998;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalState {
    /// Journal propre — montage en lecture/écriture autorisé.
    Clean,
    /// Journal contient des données non rejouées — lecture seule uniquement.
    NeedsRecovery,
    /// Erreur de lecture ou magic invalide.
    Error,
}

/// Analyse le superblock JBD2 minimal pour déterminer l'état du journal.
/// `raw` : secteur du journal (512 octets minimum).
pub fn read_journal_state(raw: &[u8]) -> JournalState {
    if raw.len() < 12 {
        return JournalState::Error;
    }
    // Offset 0 : h_magic (big-endian).
    let magic = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
    if magic != JBD2_MAGIC {
        return JournalState::Error;
    }
    // Offset 8 dans le superblock JBD2 (après h_blocktype=4, h_sequence) : s_start.
    // Superblock JBD2 v2 : offset 28 = s_start (0 = journal clean).
    if raw.len() < 32 {
        return JournalState::Error;
    }
    let s_start = u32::from_be_bytes([raw[28], raw[29], raw[30], raw[31]]);
    if s_start == 0 {
        JournalState::Clean
    } else {
        JournalState::NeedsRecovery
    }
}
