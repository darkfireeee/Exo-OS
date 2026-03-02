//! debug_interface.rs — Interface de débogage ExoFS (no_std).

use core::fmt::Write;
use super::metrics::EXOFS_METRICS;
use super::perf_counters::PERF_COUNTERS;
use super::latency_histogram::LATENCY_HIST;
use super::throughput_tracker::THROUGHPUT;
use super::space_tracker::SPACE_TRACKER;
use super::health_check::HEALTH;

/// Agrège et affiche les statistiques ExoFS dans un writer no_std.
pub struct DebugInterface;

impl DebugInterface {
    /// Résumé rapide de l'état global.
    pub fn dump_stats<W: Write>(out: &mut W) {
        let snap = EXOFS_METRICS.snapshot();
        let _ = writeln!(out, "=== ExoFS Metrics ===");
        let _ = writeln!(out, "reads:        {}", snap.reads);
        let _ = writeln!(out, "writes:       {}", snap.writes);
        let _ = writeln!(out, "cache_hits:   {}", snap.cache_hits);
        let _ = writeln!(out, "cache_misses: {}", snap.cache_misses);
        let _ = writeln!(out, "gc_runs:      {}", snap.gc_runs);
        let _ = writeln!(out, "errors:       {}", snap.errors);
        let _ = writeln!(out, "blobs_alloc:  {}", snap.blobs_allocated);
        let _ = writeln!(out, "blobs_freed:  {}", snap.blobs_freed);
        let _ = writeln!(out, "epochs_commit:{}", snap.epochs_committed);
        let _ = writeln!(out, "bytes_total:  {}", snap.bytes_written_total);
    }

    /// Compteurs de performance granulaires.
    pub fn dump_perf<W: Write>(out: &mut W) {
        let p = PERF_COUNTERS.snapshot();
        let _ = writeln!(out, "=== Perf Counters ===");
        let _ = writeln!(out, "path_resolves:     {}", p.path_resolves);
        let _ = writeln!(out, "extent_tree_ops:   {}", p.extent_tree_ops);
        let _ = writeln!(out, "blob_allocs:       {}", p.blob_allocs);
        let _ = writeln!(out, "blob_deallocs:     {}", p.blob_deallocs);
        let _ = writeln!(out, "index_lookups:     {}", p.index_lookups);
        let _ = writeln!(out, "snapshots:         {}", p.snapshot_creates);
        let _ = writeln!(out, "compress_ops:      {}", p.compression_ops);
        let _ = writeln!(out, "dedup_hits:        {}", p.dedup_hits);
        let _ = writeln!(out, "epoch_flushes:     {}", p.epoch_flushes);
        let _ = writeln!(out, "relation_inserts:  {}", p.relation_inserts);
    }

    /// Histogramme de latences.
    pub fn dump_latency<W: Write>(out: &mut W) {
        let (buckets, counts) = LATENCY_HIST.snapshot();
        let _ = writeln!(out, "=== Latency Histogram ===");
        let labels = ["<1µs", "<10µs", "<100µs", "<1ms", "<10ms", "<100ms", "<1s", ">1s"];
        for (i, &count) in counts.iter().enumerate() {
            let _ = writeln!(out, "{:>8}: {}", labels[i], count);
        }
        let _ = writeln!(out, "avg_ns:   {}", LATENCY_HIST.avg_ns());
        let _ = writeln!(out, "total:    {}", LATENCY_HIST.total_samples());
        let _ = drop(buckets); // buckets = seuils en ns, disponibles pour extension future.
    }

    /// Débit I/O.
    pub fn dump_throughput<W: Write>(out: &mut W) {
        let _ = writeln!(out, "=== Throughput ===");
        let _ = writeln!(out, "total_read_B:    {}", THROUGHPUT.total_read());
        let _ = writeln!(out, "total_written_B: {}", THROUGHPUT.total_written());
        let _ = writeln!(out, "read_bpt:        {}", THROUGHPUT.read_throughput_bpt());
        let _ = writeln!(out, "write_bpt:       {}", THROUGHPUT.write_throughput_bpt());
    }

    /// État disque.
    pub fn dump_space<W: Write>(out: &mut W) {
        let _ = writeln!(out, "=== Space ===");
        let _ = writeln!(out, "total_B:  {}", SPACE_TRACKER.total_bytes());
        let _ = writeln!(out, "used_B:   {}", SPACE_TRACKER.used_bytes());
        let _ = writeln!(out, "free_B:   {}", SPACE_TRACKER.free_bytes());
        let _ = writeln!(out, "usage%:   {}", SPACE_TRACKER.usage_pct());
    }

    /// État de santé.
    pub fn dump_health<W: Write>(out: &mut W) {
        let status = HEALTH.status();
        let _ = writeln!(out, "=== Health: {:?} ===", status);
    }

    /// Dump complet.
    pub fn dump_all<W: Write>(out: &mut W) {
        Self::dump_health(out);
        Self::dump_stats(out);
        Self::dump_perf(out);
        Self::dump_latency(out);
        Self::dump_throughput(out);
        Self::dump_space(out);
    }
}
