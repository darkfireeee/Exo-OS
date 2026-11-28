//! Memory allocator, RAM copy bandwidth, CPU compute, and crypto hash benchmarks.

use crate::bench::kernel_ipc::{avg_score, score_lower_better_ns};
use crate::bench::{now_ns, BenchCategory, BenchMetric, ModuleBenchReport};

pub fn bench_memory_cpu() -> ModuleBenchReport {
    let t_start = now_ns();
    let mut metrics = Vec::new();

    // 1. Heap alloc/free micro-benchmark (1500 x 256B buffers)
    let alloc_iters = 1_500u64;
    let t0 = now_ns();
    let mut guard = 0usize;
    for i in 0..alloc_iters {
        let mut v: Vec<u8> = Vec::with_capacity(256);
        v.push((i & 0xFF) as u8);
        v.push(0xA5);
        guard = guard.wrapping_add(v.len());
    }
    let dt_alloc = (now_ns().saturating_sub(t0)).max(1);
    let ns_alloc = dt_alloc / alloc_iters;
    metrics.push(BenchMetric {
        name: String::from("Allocateur Heap (256 o)"),
        value_text: format!("{} ns/op", ns_alloc),
        score_100: score_lower_better_ns(ns_alloc, 120, 8_000),
        detail: format!("Alloc + libération séquentielle (chk={})", guard & 0xF),
    });

    // 2. RAM sequential copy bandwidth (16 passes x 256 KiB = 4 MiB)
    let buf_len = 256 * 1024usize;
    let passes = 16u64;
    let src = vec![0x5Au8; buf_len];
    let mut dst = vec![0u8; buf_len];
    let t1 = now_ns();
    for p in 0..passes {
        dst.copy_from_slice(&src);
        dst[(p as usize) * 17 % buf_len] ^= (p as u8).wrapping_add(1);
    }
    let dt_mem = (now_ns().saturating_sub(t1)).max(1);
    let total_bytes = (buf_len as u64) * passes;
    // MiB/s = bytes * 1e9 / (dt_ns * 1048576)
    let mib_s = ((total_bytes as u128 * 1_000_000_000) / ((dt_mem as u128) * 1_048_576)) as u64;
    let score_mem = ((mib_s * 100) / 2500).clamp(35, 100) as u32;
    metrics.push(BenchMetric {
        name: String::from("Bande passante RAM"),
        value_text: format!("{} Mio/s", mib_s.max(1)),
        score_100: score_mem,
        detail: format!("Copie 4 Mio (chk={})", dst[0]),
    });

    // 3. Crypto ARX stream mix throughput (ChaCha-like quarter-round over 256 KiB)
    let t2 = now_ns();
    let mut st = [0x6170_7865u32, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];
    for chunk in dst.chunks_exact(16) {
        let w0 = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let w1 = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        st[0] = st[0].wrapping_add(st[1] ^ w0);
        st[3] = (st[3] ^ st[0]).rotate_left(16);
        st[2] = st[2].wrapping_add(st[3] ^ w1);
        st[1] = (st[1] ^ st[2]).rotate_left(12);
    }
    let dt_cry = (now_ns().saturating_sub(t2)).max(1);
    let cry_mib_s = ((buf_len as u128 * 1_000_000_000) / ((dt_cry as u128) * 1_048_576)) as u64;
    let score_cry = ((cry_mib_s * 100) / 400).clamp(35, 100) as u32;
    metrics.push(BenchMetric {
        name: String::from("Débit Crypto / Hachage"),
        value_text: format!("{} Mio/s", cry_mib_s.max(1)),
        score_100: score_cry,
        detail: format!("Mixage ARX 256 Kio (digest {:04x})", st[0] & 0xFFFF),
    });

    // 4. CPU ALU integer compute (200k ops)
    let alu_iters = 200_000u64;
    let t3 = now_ns();
    let mut x = 0x1234_5678_9abc_def0u64;
    for i in 0..alu_iters {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(i | 1);
        x ^= x >> 29;
    }
    let dt_alu = (now_ns().saturating_sub(t3)).max(1);
    let mops = ((alu_iters as u128 * 1_000) / (dt_alu as u128).max(1)) as u64;
    let score_alu = ((mops * 100) / 180).clamp(40, 100) as u32;
    metrics.push(BenchMetric {
        name: String::from("Calcul Entier CPU"),
        value_text: format!("{} Mops/s", mops.max(1)),
        score_100: score_alu,
        detail: format!("200k itérations ALU (chk={:x})", x & 0xFF),
    });

    let score = avg_score(&metrics);
    ModuleBenchReport {
        category: BenchCategory::MemoryCpu,
        score_100: score,
        elapsed_us: (now_ns().saturating_sub(t_start)) / 1000,
        metrics,
    }
}
