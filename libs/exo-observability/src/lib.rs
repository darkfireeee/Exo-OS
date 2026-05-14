#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

use log::Level;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExoLogRecord {
    pub level: Level,
    pub target: &'static str,
    pub code: u32,
}

pub const OBSERVABILITY_PORTS: &[(&str, &str, &str)] = &[
    ("log", "log-upstream", "kernel/Ring services facade"),
    ("tracing", "tracing-upstream", "Ring3 structured telemetry"),
];

pub const fn severity_score(level: Level) -> u8 {
    match level {
        Level::Error => 5,
        Level::Warn => 4,
        Level::Info => 3,
        Level::Debug => 2,
        Level::Trace => 1,
    }
}

pub fn observability_stress_signature(iterations: u32) -> u64 {
    let levels = [
        Level::Error,
        Level::Warn,
        Level::Info,
        Level::Debug,
        Level::Trace,
    ];
    let mut acc = 0x4558_4f4c_4f47_u64;
    for i in 0..iterations.max(1) {
        let level = levels[i as usize % levels.len()];
        let rec = ExoLogRecord {
            level,
            target: "exo",
            code: i,
        };
        acc = acc.rotate_left(7) ^ rec.code as u64 ^ severity_score(rec.level) as u64;
    }
    acc
}
