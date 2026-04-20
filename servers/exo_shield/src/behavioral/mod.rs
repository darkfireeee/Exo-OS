//! # Behavioral Module — Analyse comportementale des processus
//!
//! Module principal pour l'analyse comportementale :
//! - `anomaly` : détection d'anomalies avec baselines et scores de déviation
//! - `heuristic` : analyse heuristique avec règles et scoring
//! - `profiler` : profilage des processus (syscalls, mémoire, réseau, IPC)
//! - `sequence` : analyse de séquences comportementales (machine à états)

pub mod anomaly;
pub mod heuristic;
pub mod profiler;
pub mod sequence;

/// Initialise tous les sous-modules comportementaux.
pub fn behavioral_init() {
    anomaly::anomaly_init();
    heuristic::heuristic_init();
    profiler::profiler_init();
    sequence::sequence_init();
}
