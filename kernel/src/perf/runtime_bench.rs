//! Microbenchmarks exécutés au boot pour mesurer les chemins critiques
//! - IPC inline (Fusion Rings si activé, sinon Channel standard)
//! - Round-trip via appels système (sys_ipc_send/sys_ipc_recv)

use alloc::vec::Vec;
use alloc::string::String;
use crate::perf::bench_framework::{BenchStats, BenchmarkSuite, rdtsc_precise, calibrate_tsc_frequency};

pub fn run_startup_microbenchmarks() {
    crate::println!("\n[PERF] Démarrage des microbenchmarks runtime...");
    #[cfg(feature = "fusion_rings")]
    crate::println!("[PERF] Mode IPC: Fusion Rings (fast path)");
    #[cfg(not(feature = "fusion_rings"))]
    crate::println!("[PERF] Mode IPC: Canal standard (fallback)");
    let mut suite = BenchmarkSuite::new(String::from("Startup Microbenches"));

    crate::println!("[PERF] Bench IPC inline: start");
    let ipc_stats = bench_ipc_inline_roundtrip(1_000);
    crate::println!("[PERF] Bench IPC inline: done");
    suite.add_result(ipc_stats);

    // Impression compacte immédiate (garantit visibilité même si rendu ASCII échoue)
    crate::println!("[MICROBENCH][IPC Inline] mean=0x{:x} min=0x{:x} max=0x{:x} p50=0x{:x} p95=0x{:x} p99=0x{:x} (cycles)",
        suite.results[0].mean,
        suite.results[0].min,
        suite.results[0].max,
        suite.results[0].p50,
        suite.results[0].p95,
        suite.results[0].p99);

    crate::println!("[PERF] Bench Syscall+IPC: start");
    let syscall_stats = bench_syscall_ipc_roundtrip(1_000);
    crate::println!("[PERF] Bench Syscall+IPC: done");
    suite.add_result(syscall_stats);

    crate::println!("[MICROBENCH][Syscall IPC] mean=0x{:x} min=0x{:x} max=0x{:x} p50=0x{:x} p95=0x{:x} p99=0x{:x} (cycles)",
        suite.results[1].mean,
        suite.results[1].min,
        suite.results[1].max,
        suite.results[1].p50,
        suite.results[1].p95,
        suite.results[1].p99);

    // Afficher un résumé concis des résultats
    suite.print_results();
    
    // Export CSV des résultats pour analyse automatique
    crate::println!("\n[CSV_EXPORT_START]");
    let csv = suite.to_csv();
    for line in csv.lines() {
        crate::println!("{}", line);
    }
    crate::println!("[CSV_EXPORT_END]\n");

    // Option: sortie automatique de QEMU après les microbenchmarks
    #[cfg(feature = "bench_auto_exit")]
    {
        crate::println!("[PERF] Fin microbenchmarks → exit QEMU (isa-debug-exit)");
        // Brève pause pour laisser le temps au port série de vider son buffer
        for _ in 0..100_000 { unsafe { core::arch::asm!("pause") } }
        self::qemu_exit_success();
    }
}

fn bench_ipc_inline_roundtrip(iterations: usize) -> BenchStats {
    // S'assurer qu'un canal de test existe et qu'il est potentiellement mappé au FastChannel
    let chan_id = crate::ipc::create_channel("perf_test", 64).unwrap_or(1);

    let mut samples: Vec<u64> = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        #[cfg(feature = "fusion_rings")]
        {
            // Message inline (≤ INLINE_SIZE)
            let msg: &[u8] = b"abcdefgh"; // 8 octets
            let start = rdtsc_precise();
            let _ = crate::ipc::send_fast_by_id(chan_id, msg);
            // Lecture immédiate (boucle locale sur le même canal)
            let _ = crate::ipc::receive_fast_by_id(chan_id);
            let end = rdtsc_precise();
            samples.push(end - start);
        }

        #[cfg(not(feature = "fusion_rings"))]
        {
            let msg_buf = alloc::vec::Vec::from(&b"abcdefgh"[..]);
            let msg = crate::ipc::message::Message::new_buffered(0, 0, 0, msg_buf);
            let start = rdtsc_precise();
            let _ = crate::ipc::send_message(chan_id, msg);
            let _ = crate::ipc::receive_message(chan_id);
            let end = rdtsc_precise();
            samples.push(end - start);
        }
    }

    BenchStats::new(String::from("IPC Inline Roundtrip"), samples)
}

fn bench_syscall_ipc_roundtrip(iterations: usize) -> BenchStats {
    // S'assurer qu'un canal de test existe
    let chan_id = crate::ipc::create_channel("perf_test", 64).unwrap_or(1);
    let mut samples: Vec<u64> = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let msg: [u8; 8] = *b"12345678";
        let mut recv_buf = [0u8; 64];

        let send_args = crate::syscall::SyscallArgs {
            rdi: chan_id as u64,
            rsi: msg.as_ptr() as u64,
            rdx: msg.len() as u64,
            r10: 0,
            r8: 0,
            r9: 0,
        };
        let recv_args = crate::syscall::SyscallArgs {
            rdi: chan_id as u64,
            rsi: recv_buf.as_mut_ptr() as u64,
            rdx: recv_buf.len() as u64,
            r10: 0,
            r8: 0,
            r9: 0,
        };

        let start = rdtsc_precise();
        let _ = crate::syscall::sys_ipc_send(send_args);
        let _ = crate::syscall::sys_ipc_recv(recv_args);
        let end = rdtsc_precise();

        samples.push(end - start);
    }

    BenchStats::new(String::from("Syscall IPC Roundtrip"), samples)
}

#[cfg(all(feature = "bench_auto_exit", target_arch = "x86_64"))]
#[inline(never)]
fn qemu_exit_success() -> ! {
    use x86_64::instructions::port::Port;
    unsafe {
        let mut port = Port::new(0xf4);
        // Code 0x10 = succès (aligné avec tests)
        port.write(0x10u32);
    }
    loop {}
}
