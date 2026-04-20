//! Feature extraction for the exo_shield ML pipeline.
//!
//! Defines a 32-element `FeatureVector`, normalisation helpers, and
//! extraction of features from raw process-behaviour data.

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of features per vector.
pub const FEATURE_COUNT: usize = 32;

/// Index constants for well-known features.
pub const FEAT_SYSCALL_RATE: usize = 0;
pub const FEAT_FILE_OPEN_RATE: usize = 1;
pub const FEAT_FILE_WRITE_RATE: usize = 2;
pub const FEAT_NET_CONNECT_RATE: usize = 3;
pub const FEAT_NET_BYTES_SENT: usize = 4;
pub const FEAT_NET_BYTES_RECV: usize = 5;
pub const FEAT_CPU_USAGE: usize = 6;
pub const FEAT_MEM_USAGE: usize = 7;
pub const FEAT_CHILD_FORK_RATE: usize = 8;
pub const FEAT_EXEC_RATE: usize = 9;
pub const FEAT_SIGNAL_RATE: usize = 10;
pub const FEAT_IPC_MSG_RATE: usize = 11;
pub const FEAT_PRIV_ESCALATION_ATTEMPTS: usize = 12;
pub const FEAT_DENIED_SYSCALL_COUNT: usize = 13;
pub const FEAT_FILE_PERMISSION_CHANGES: usize = 14;
pub const FEAT_SUSPICIOUS_PATH_ACCESS: usize = 15;
pub const FEAT_DNS_QUERY_RATE: usize = 16;
pub const FEAT_DNS_UNIQUE_DOMAINS: usize = 17;
pub const FEAT_NET_PORT_SCAN_SCORE: usize = 18;
pub const FEAT_BIND_ATTEMPTS: usize = 19;
pub const FEAT_SHARED_MEM_CREATE: usize = 20;
pub const FEAT_THREAD_CREATE_RATE: usize = 21;
pub const FEAT_SYSCALL_DIVERSITY: usize = 22;
pub const FEAT_FILE_DELETE_RATE: usize = 23;
pub const FEAT_PROCESS_DURATION: usize = 24;
pub const FEAT_RENICE_ATTEMPTS: usize = 25;
pub const FEAT_MODULE_LOAD_RATE: usize = 26;
pub const FEAT_RAW_SOCKET_USE: usize = 27;
pub const FEAT_CHROOT_ATTEMPTS: usize = 28;
pub const FEAT_CLONE_NAMESPACE: usize = 29;
pub const FEAT_PTRACE_USE: usize = 30;
pub const FEAT_ANOMALY_RUNNING_AVG: usize = 31;

// ---------------------------------------------------------------------------
// Feature vector
// ---------------------------------------------------------------------------

/// A fixed-size vector of 32 features stored as fixed-point i32 values.
/// The raw values use a Q16.16 fixed-point representation so that
/// normalised values in [0.0, 1.0] map to [0, 65536].
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct FeatureVector {
    /// Feature values in Q16.16 fixed-point.
    values: [i32; FEATURE_COUNT],
    /// Whether the vector has been normalised.
    normalised: bool,
}

/// One in Q16.16.
const FP_ONE: i32 = 1 << 16;

impl FeatureVector {
    /// Create a zero-valued feature vector.
    pub const fn zero() -> Self {
        Self {
            values: [0i32; FEATURE_COUNT],
            normalised: false,
        }
    }

    /// Create from an array of raw i32 values.
    pub const fn from_raw(values: [i32; FEATURE_COUNT]) -> Self {
        Self {
            values,
            normalised: false,
        }
    }

    /// Get a feature value by index.  Returns 0 for out-of-range indices.
    pub fn get(&self, idx: usize) -> i32 {
        if idx < FEATURE_COUNT {
            self.values[idx]
        } else {
            0
        }
    }

    /// Set a feature value by index.  No-op for out-of-range indices.
    pub fn set(&mut self, idx: usize, val: i32) {
        if idx < FEATURE_COUNT {
            self.values[idx] = val;
        }
    }

    /// Return a reference to the underlying array.
    pub fn values(&self) -> &[i32; FEATURE_COUNT] {
        &self.values
    }

    /// Convert a Q16.16 value to an approximate f32 equivalent (for
    /// documentation / debug; actual arithmetic stays in fixed-point).
    pub fn to_f32(idx: usize) -> f32 {
        // This is only used for human-readable output; in no_std we
        // return a rough approximation using integer math.
        // For actual computation, use the fixed-point values directly.
        0.0 // f32 not available in no_std; placeholder for documentation
    }

    /// Whether this vector has been normalised.
    pub fn is_normalised(&self) -> bool {
        self.normalised
    }

    /// Normalise features to [0, FP_ONE] range using min-max scaling.
    /// The min and max arrays must be provided by the caller (they are
    /// typically derived from training data statistics).
    pub fn normalise_minmax(&mut self, min: &[i32; FEATURE_COUNT], max: &[i32; FEATURE_COUNT]) {
        for i in 0..FEATURE_COUNT {
            let range = max[i].saturating_sub(min[i]);
            if range == 0 {
                self.values[i] = 0;
                continue;
            }
            let shifted = self.values[i].saturating_sub(min[i]);
            // Q16.16 division: (shifted << 16) / range
            // Use saturating arithmetic to avoid overflow.
            let numerator = (shifted as i64) << 16;
            let result = (numerator / (range as i64)) as i32;
            self.values[i] = result.clamp(0, FP_ONE);
        }
        self.normalised = true;
    }

    /// Z-score normalisation (subtract mean, divide by stddev) in Q16.16.
    /// `mean` and `stddev` arrays are provided by the caller.
    pub fn normalise_zscore(
        &mut self,
        mean: &[i32; FEATURE_COUNT],
        stddev: &[i32; FEATURE_COUNT],
    ) {
        for i in 0..FEATURE_COUNT {
            if stddev[i] == 0 {
                self.values[i] = 0;
                continue;
            }
            let centered = self.values[i].saturating_sub(mean[i]);
            // Q16.16 division: centered / stddev (both in Q16.16)
            let result = ((centered as i64) << 16) / (stddev[i] as i64);
            self.values[i] = result as i32;
        }
        self.normalised = true;
    }

    /// Compute the L2 norm squared (sum of squares) in Q16.16.
    pub fn l2_norm_sq(&self) -> i64 {
        let mut sum: i64 = 0;
        for i in 0..FEATURE_COUNT {
            // Each value is Q16.16; product is Q32.32; shift back.
            let v = self.values[i] as i64;
            sum += (v * v) >> 16;
        }
        sum
    }
}

// ---------------------------------------------------------------------------
// Raw process behaviour data (input for feature extraction)
// ---------------------------------------------------------------------------

/// Aggregated process-behaviour counters from which features are derived.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ProcessBehaviourData {
    /// Syscalls per tick.
    pub syscall_rate: i32,
    /// File opens per tick.
    pub file_open_rate: i32,
    /// File writes per tick.
    pub file_write_rate: i32,
    /// Network connections per tick.
    pub net_connect_rate: i32,
    /// Network bytes sent (raw counter).
    pub net_bytes_sent: i32,
    /// Network bytes received (raw counter).
    pub net_bytes_recv: i32,
    /// CPU usage fraction (Q16.16).
    pub cpu_usage: i32,
    /// Memory usage fraction (Q16.16).
    pub mem_usage: i32,
    /// Fork / clone rate per tick.
    pub child_fork_rate: i32,
    /// Exec rate per tick.
    pub exec_rate: i32,
    /// Signals sent per tick.
    pub signal_rate: i32,
    /// IPC messages per tick.
    pub ipc_msg_rate: i32,
    /// Privilege escalation attempts.
    pub priv_escalation_attempts: i32,
    /// Denied syscall count.
    pub denied_syscall_count: i32,
    /// File permission change count.
    pub file_permission_changes: i32,
    /// Suspicious path access count.
    pub suspicious_path_access: i32,
    /// DNS queries per tick.
    pub dns_query_rate: i32,
    /// Unique DNS domains queried.
    pub dns_unique_domains: i32,
    /// Port scan heuristic score (0–65536).
    pub port_scan_score: i32,
    /// bind() attempts.
    pub bind_attempts: i32,
    /// Shared memory creates per tick.
    pub shm_create_rate: i32,
    /// Thread creation rate.
    pub thread_create_rate: i32,
    /// Number of distinct syscall numbers used.
    pub syscall_diversity: i32,
    /// File delete rate.
    pub file_delete_rate: i32,
    /// Process lifetime in ticks.
    pub process_duration: i32,
    /// renice / setpriority attempts.
    pub renice_attempts: i32,
    /// Module (kernel module) load attempts.
    pub module_load_rate: i32,
    /// Raw socket usage count.
    pub raw_socket_use: i32,
    /// chroot attempts.
    pub chroot_attempts: i32,
    /// clone with new namespace flags.
    pub clone_namespace: i32,
    /// ptrace use count.
    pub ptrace_use: i32,
    /// Running average anomaly score (Q16.16).
    pub anomaly_running_avg: i32,
}

impl ProcessBehaviourData {
    /// Zero-initialised data.
    pub const fn zero() -> Self {
        Self {
            syscall_rate: 0,
            file_open_rate: 0,
            file_write_rate: 0,
            net_connect_rate: 0,
            net_bytes_sent: 0,
            net_bytes_recv: 0,
            cpu_usage: 0,
            mem_usage: 0,
            child_fork_rate: 0,
            exec_rate: 0,
            signal_rate: 0,
            ipc_msg_rate: 0,
            priv_escalation_attempts: 0,
            denied_syscall_count: 0,
            file_permission_changes: 0,
            suspicious_path_access: 0,
            dns_query_rate: 0,
            dns_unique_domains: 0,
            port_scan_score: 0,
            bind_attempts: 0,
            shm_create_rate: 0,
            thread_create_rate: 0,
            syscall_diversity: 0,
            file_delete_rate: 0,
            process_duration: 0,
            renice_attempts: 0,
            module_load_rate: 0,
            raw_socket_use: 0,
            chroot_attempts: 0,
            clone_namespace: 0,
            ptrace_use: 0,
            anomaly_running_avg: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Feature extractor
// ---------------------------------------------------------------------------

/// Extracts a `FeatureVector` from `ProcessBehaviourData`.
pub struct FeatureExtractor;

impl FeatureExtractor {
    /// Extract raw features from process behaviour data.
    /// The resulting vector is *not* normalised.
    pub fn extract(data: &ProcessBehaviourData) -> FeatureVector {
        let values: [i32; FEATURE_COUNT] = [
            data.syscall_rate,
            data.file_open_rate,
            data.file_write_rate,
            data.net_connect_rate,
            data.net_bytes_sent,
            data.net_bytes_recv,
            data.cpu_usage,
            data.mem_usage,
            data.child_fork_rate,
            data.exec_rate,
            data.signal_rate,
            data.ipc_msg_rate,
            data.priv_escalation_attempts,
            data.denied_syscall_count,
            data.file_permission_changes,
            data.suspicious_path_access,
            data.dns_query_rate,
            data.dns_unique_domains,
            data.port_scan_score,
            data.bind_attempts,
            data.shm_create_rate,
            data.thread_create_rate,
            data.syscall_diversity,
            data.file_delete_rate,
            data.process_duration,
            data.renice_attempts,
            data.module_load_rate,
            data.raw_socket_use,
            data.chroot_attempts,
            data.clone_namespace,
            data.ptrace_use,
            data.anomaly_running_avg,
        ];
        FeatureVector::from_raw(values)
    }

    /// Extract *and* normalise using min-max scaling.
    pub fn extract_normalised(
        data: &ProcessBehaviourData,
        min: &[i32; FEATURE_COUNT],
        max: &[i32; FEATURE_COUNT],
    ) -> FeatureVector {
        let mut fv = Self::extract(data);
        fv.normalise_minmax(min, max);
        fv
    }

    /// Compute a simple anomaly score from a feature vector by summing
    /// the values of "suspicious" feature indices, weighted.
    ///
    /// Returns the score in Q16.16.
    pub fn anomaly_score(fv: &FeatureVector) -> i32 {
        // Weights for each feature (Q16.16); higher = more suspicious.
        const WEIGHTS: [i32; FEATURE_COUNT] = [
            0x0001_0000, // syscall_rate          ×1
            0x0000_8000, // file_open_rate        ×0.5
            0x0000_C000, // file_write_rate       ×0.75
            0x0001_4000, // net_connect_rate      ×1.25
            0x0000_4000, // net_bytes_sent        ×0.25
            0x0000_4000, // net_bytes_recv        ×0.25
            0x0000_8000, // cpu_usage             ×0.5
            0x0000_8000, // mem_usage             ×0.5
            0x0001_8000, // child_fork_rate       ×1.5
            0x0001_8000, // exec_rate             ×1.5
            0x0001_0000, // signal_rate           ×1
            0x0000_C000, // ipc_msg_rate          ×0.75
            0x0003_0000, // priv_escalation       ×3
            0x0002_0000, // denied_syscall        ×2
            0x0001_C000, // file_perm_changes     ×1.75
            0x0002_4000, // suspicious_path       ×2.25
            0x0001_4000, // dns_query_rate        ×1.25
            0x0001_C000, // dns_unique_domains    ×1.75
            0x0003_0000, // port_scan             ×3
            0x0002_0000, // bind_attempts         ×2
            0x0001_4000, // shm_create            ×1.25
            0x0001_0000, // thread_create         ×1
            0x0000_C000, // syscall_diversity     ×0.75
            0x0001_C000, // file_delete           ×1.75
            0x0000_4000, // process_duration      ×0.25
            0x0002_8000, // renice                ×2.5
            0x0003_0000, // module_load           ×3
            0x0003_0000, // raw_socket            ×3
            0x0003_0000, // chroot                ×3
            0x0002_8000, // clone_namespace       ×2.5
            0x0003_0000, // ptrace                ×3
            0x0001_0000, // anomaly_avg           ×1
        ];

        let mut score: i64 = 0;
        for i in 0..FEATURE_COUNT {
            let v = fv.get(i) as i64;
            let w = WEIGHTS[i] as i64;
            // Q16.16 × Q16.16 = Q32.32; shift back by 16.
            score += (v * w) >> 16;
        }
        // Clamp to i32 range.
        score.clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    /// Derive min/max arrays from a batch of behaviour data by computing
    /// per-feature min and max across all samples.
    pub fn compute_minmax(
        samples: &[ProcessBehaviourData],
        min_out: &mut [i32; FEATURE_COUNT],
        max_out: &mut [i32; FEATURE_COUNT],
    ) {
        // Initialise min to MAX, max to MIN
        for i in 0..FEATURE_COUNT {
            min_out[i] = i32::MAX;
            max_out[i] = i32::MIN;
        }
        for sample in samples {
            let fv = Self::extract(sample);
            for i in 0..FEATURE_COUNT {
                let v = fv.get(i);
                if v < min_out[i] {
                    min_out[i] = v;
                }
                if v > max_out[i] {
                    max_out[i] = v;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_vector_zero() {
        let fv = FeatureVector::zero();
        for i in 0..FEATURE_COUNT {
            assert_eq!(fv.get(i), 0);
        }
    }

    #[test]
    fn feature_vector_set_get() {
        let mut fv = FeatureVector::zero();
        fv.set(FEAT_CPU_USAGE, 0x8000); // ~0.5 in Q16.16
        assert_eq!(fv.get(FEAT_CPU_USAGE), 0x8000);
        assert_eq!(fv.get(FEAT_MEM_USAGE), 0); // unchanged
    }

    #[test]
    fn normalise_minmax() {
        let mut fv = FeatureVector::from_raw([100i32; FEATURE_COUNT]);
        let min = [0i32; FEATURE_COUNT];
        let max = [200i32; FEATURE_COUNT];
        fv.normalise_minmax(&min, &max);
        assert!(fv.is_normalised());
        // 100 is the midpoint → should normalise to ~0.5 in Q16.16 = 32768
        for i in 0..FEATURE_COUNT {
            let v = fv.get(i);
            assert!(v > 32000 && v < 34000, "feature {} = {}", i, v);
        }
    }

    #[test]
    fn extract_from_behaviour() {
        let mut data = ProcessBehaviourData::zero();
        data.syscall_rate = 500;
        data.denied_syscall_count = 3;
        data.ptrace_use = 1;
        let fv = FeatureExtractor::extract(&data);
        assert_eq!(fv.get(FEAT_SYSCALL_RATE), 500);
        assert_eq!(fv.get(FEAT_DENIED_SYSCALL_COUNT), 3);
        assert_eq!(fv.get(FEAT_PTRACE_USE), 1);
    }

    #[test]
    fn anomaly_score_higher_with_suspicious() {
        let mut benign = ProcessBehaviourData::zero();
        benign.syscall_rate = 10;
        benign.cpu_usage = 0x4000;

        let mut suspicious = ProcessBehaviourData::zero();
        suspicious.ptrace_use = 5;
        suspicious.priv_escalation_attempts = 3;
        suspicious.port_scan_score = 100;

        let fv_benign = FeatureExtractor::extract(&benign);
        let fv_suspicious = FeatureExtractor::extract(&suspicious);

        let score_benign = FeatureExtractor::anomaly_score(&fv_benign);
        let score_suspicious = FeatureExtractor::anomaly_score(&fv_suspicious);
        assert!(score_suspicious > score_benign);
    }
}
