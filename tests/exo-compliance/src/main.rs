mod raw;

use raw::*;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier, Condvar, Mutex, Once, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SELF_BIN: &str = "/bin/exo-compliance-runner";

struct CaseSpec {
    tier: &'static str,
    name: &'static str,
    expect_sigsegv: bool,
}

const CASES: &[CaseSpec] = &[
    CaseSpec { tier: "T0", name: "t0_boot_layout", expect_sigsegv: false },
    CaseSpec { tier: "T1", name: "t1_core_runtime", expect_sigsegv: false },
    CaseSpec { tier: "T2", name: "t2_threads_tls_sync", expect_sigsegv: false },
    CaseSpec { tier: "T2", name: "t2_signals_fpu_mask", expect_sigsegv: false },
    CaseSpec { tier: "T2", name: "t2_guard_page_sigsegv", expect_sigsegv: true },
    CaseSpec { tier: "T3", name: "t3_fs_honest_posix", expect_sigsegv: false },
    CaseSpec { tier: "T3", name: "t3_flock_multiprocess", expect_sigsegv: false },
    CaseSpec { tier: "T4", name: "t4_unix_dgram_scm_rights", expect_sigsegv: false },
    CaseSpec { tier: "T4", name: "t4_epoll_event_driven", expect_sigsegv: false },
    CaseSpec { tier: "T4", name: "t4_wayland_live_ipc", expect_sigsegv: false },
    CaseSpec { tier: "T5", name: "t5_tcp_loopback_1mb_and_epoll", expect_sigsegv: false },
    CaseSpec { tier: "T5", name: "t5_udp_large_dns_hostname", expect_sigsegv: false },
    CaseSpec { tier: "T6", name: "t6_dynamic_loader_and_musl", expect_sigsegv: false },
];

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() >= 3 && args[1] == "--case" {
        let name = args[2].as_str();
        let res = run_case(name);
        match res {
            Ok(()) => std::process::exit(0),
            Err(msg) => {
                eprintln!("FAIL {name}: {msg}");
                std::process::exit(1);
            }
        }
    }
    if args.len() >= 3 && args[1] == "--helper" {
        run_helper(&args[2..]);
        return;
    }
    if args.len() >= 2 && args[1] == "--pie-self-check" {
        match run_pie_self_check() {
            Ok(msg) => {
                println!("{msg}");
                std::process::exit(0);
            }
            Err(err) => {
                eprintln!("[EXO_PIE] FAIL: {err}");
                std::process::exit(1);
            }
        }
    }

    let exit_qemu = args.iter().any(|a| a == "--exit-qemu");
    run_supervisor(exit_qemu);
}

fn run_supervisor(exit_qemu: bool) {
    let _ = fs::create_dir_all("/tmp");
    exo_log_line("[EXO_TEST] START SUITE=exo-std-compliance");
    let mut pass = 0usize;
    let mut fail = 0usize;

    for spec in CASES {
        let start = Instant::now();
        if spec.expect_sigsegv {
            let output = Command::new(SELF_BIN)
                .arg("--case")
                .arg(spec.name)
                .output();
            let ms = start.elapsed().as_millis();
            match output {
                Ok(out) => {
                    let status_str = out.status.to_string();
                    let ok = status_str.contains("signal: 11") || out.status.code() == Some(139);
                    if ok {
                        pass += 1;
                        exo_log_line(&format!(
                            "[EXO_TEST] CASE={} TIER={} RESULT=PASS DURATION_MS={}",
                            spec.name, spec.tier, ms
                        ));
                    } else {
                        fail += 1;
                        let stderr_s = String::from_utf8_lossy(&out.stderr);
                        let detail = stderr_s.trim().replace('\n', " | ");
                        exo_log_line(&format!(
                            "[EXO_TEST] CASE={} TIER={} RESULT=FAIL DURATION_MS={} STATUS=\"{}\" DETAIL=\"{}\"",
                            spec.name, spec.tier, ms, status_str, detail
                        ));
                    }
                }
                Err(e) => {
                    fail += 1;
                    exo_log_line(&format!(
                        "[EXO_TEST] CASE={} TIER={} RESULT=FAIL DURATION_MS={} DETAIL=\"spawn failed: {}\"",
                        spec.name, spec.tier, ms, e
                    ));
                }
            }
        } else {
            let res = run_case(spec.name);
            let ms = start.elapsed().as_millis();
            match res {
                Ok(()) => {
                    pass += 1;
                    exo_log_line(&format!(
                        "[EXO_TEST] CASE={} TIER={} RESULT=PASS DURATION_MS={}",
                        spec.name, spec.tier, ms
                    ));
                }
                Err(detail) => {
                    fail += 1;
                    let clean = detail.trim().replace('\n', " | ");
                    exo_log_line(&format!(
                        "[EXO_TEST] CASE={} TIER={} RESULT=FAIL DURATION_MS={} DETAIL=\"{}\"",
                        spec.name, spec.tier, ms, clean
                    ));
                }
            }
        }
    }

    exo_log_line(&format!(
        "[EXO_TEST] SUMMARY PASS={} FAIL={} TOTAL={}",
        pass,
        fail,
        CASES.len()
    ));

    if exit_qemu {
        compliance_exit_qemu(fail == 0);
    }
    std::process::exit(if fail == 0 { 0 } else { 1 });
}

fn run_helper(args: &[String]) {
    match args.first().map(|s| s.as_str()) {
        Some("echo-pipe") => {
            if let Ok(v) = env::var("EXO_COMP_ENV") {
                let _ = writeln!(io::stderr(), "ENV={v}");
            }
            let mut stdin = io::stdin().lock();
            let mut stdout = io::stdout().lock();
            let mut buf = [0u8; 8192];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if stdout.write_all(&buf[..n]).is_err() {
                            std::process::exit(2);
                        }
                    }
                    Err(_) => std::process::exit(3),
                }
            }
            let _ = stdout.flush();
            std::process::exit(0);
        }
        Some("exit42") => std::process::exit(42),
        Some("flock-child") => {
            let path = args.get(1).expect("flock path");
            let file = OpenOptions::new().read(true).write(true).open(path).expect("open lock");
            let fd = file.as_raw_fd() as u64;
            let nb = unsafe { syscall2(SYS_FLOCK, fd, LOCK_EX | LOCK_NB) };
            if nb != EAGAIN {
                eprintln!("expected EAGAIN (-11) on LOCK_EX|LOCK_NB, got {nb}");
                std::process::exit(2);
            }
            println!("BLOCKING");
            let _ = io::stdout().flush();
            let blk = unsafe { syscall2(SYS_FLOCK, fd, LOCK_EX) };
            if blk != 0 {
                eprintln!("blocking flock failed: {blk}");
                std::process::exit(3);
            }
            let _ = unsafe { syscall2(SYS_FLOCK, fd, LOCK_UN) };
            std::process::exit(0);
        }
        _ => std::process::exit(99),
    }
}

fn run_case(name: &str) -> Result<(), String> {
    match name {
        "t0_boot_layout" => t0_boot_layout(),
        "t1_core_runtime" => t1_core_runtime(),
        "t2_threads_tls_sync" => t2_threads_tls_sync(),
        "t2_signals_fpu_mask" => t2_signals_fpu_mask(),
        "t2_guard_page_sigsegv" => t2_guard_page_sigsegv(),
        "t3_fs_honest_posix" => t3_fs_honest_posix(),
        "t3_flock_multiprocess" => t3_flock_multiprocess(),
        "t4_unix_dgram_scm_rights" => t4_unix_dgram_scm_rights(),
        "t4_epoll_event_driven" => t4_epoll_event_driven(),
        "t4_wayland_live_ipc" => t4_wayland_live_ipc(),
        "t5_tcp_loopback_1mb_and_epoll" => t5_tcp_loopback_1mb_and_epoll(),
        "t5_udp_large_dns_hostname" => t5_udp_large_dns_hostname(),
        "t6_dynamic_loader_and_musl" => t6_dynamic_loader_and_musl(),
        other => Err(format!("unknown case: {other}")),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// T0: Boot & rootfs integrity
// ─────────────────────────────────────────────────────────────────────────────

fn t0_boot_layout() -> Result<(), String> {
    let required = [
        "/sbin/exo-init-server",
        "/sbin/exo-network-server",
        "/sbin/exo-fb-server",
        "/sbin/exo-loopback-driver",
        "/bin/exosh",
        "/lib/ld-exo.so",
        "/bin/p3dyn",
        "/bin/p3musl",
        "/run/wayland-0",
        "/etc/resolv.conf",
    ];
    for p in required {
        if !Path::new(p).exists() {
            return Err(format!("missing required path: {p}"));
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// T1: Core runtime (args, env, Command >128KiB pipes, clocks, getrandom, alloc)
// ─────────────────────────────────────────────────────────────────────────────

fn t1_core_runtime() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();
    if args.is_empty() || !args[0].contains("exo-compliance-runner") {
        return Err(format!("unexpected argv: {args:?}"));
    }

    // Env set/get/remove + child inheritance
    unsafe {
        env::set_var("EXO_COMP_ENV", "ok_42");
    }
    if env::var("EXO_COMP_ENV").as_deref() != Ok("ok_42") {
        return Err("env::var mismatch".into());
    }

    // Command bidirectional pipe > 128 KiB (160 KiB)
    const PAYLOAD_LEN: usize = 160 * 1024;
    let payload: Vec<u8> = (0..PAYLOAD_LEN).map(|i| ((i * 31 + 17) & 0xff) as u8).collect();
    let mut child = Command::new(SELF_BIN)
        .args(["--helper", "echo-pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn echo-pipe: {e}"))?;

    let mut c_in = child.stdin.take().ok_or("no stdin")?;
    let mut c_out = child.stdout.take().ok_or("no stdout")?;
    let mut c_err = child.stderr.take().ok_or("no stderr")?;

    let tx_data = payload.clone();
    let writer = thread::spawn(move || -> io::Result<()> {
        c_in.write_all(&tx_data)?;
        c_in.flush()?;
        drop(c_in);
        Ok(())
    });

    let err_reader = thread::spawn(move || -> io::Result<String> {
        let mut s = String::new();
        c_err.read_to_string(&mut s)?;
        Ok(s)
    });

    let mut rx_data = Vec::with_capacity(PAYLOAD_LEN);
    c_out
        .read_to_end(&mut rx_data)
        .map_err(|e| format!("read stdout: {e}"))?;

    writer
        .join()
        .map_err(|_| "writer panicked".to_string())?
        .map_err(|e| format!("writer err: {e}"))?;
    let stderr_text = err_reader
        .join()
        .map_err(|_| "stderr reader panicked".to_string())?
        .map_err(|e| format!("stderr err: {e}"))?;
    let st = child.wait().map_err(|e| format!("wait: {e}"))?;
    if !st.success() {
        return Err(format!("echo-pipe exited with {st}"));
    }
    if rx_data.len() != PAYLOAD_LEN || rx_data != payload {
        return Err(format!(
            "pipe echo mismatch: got {} bytes, expected {}",
            rx_data.len(),
            PAYLOAD_LEN
        ));
    }
    if !stderr_text.contains("ENV=ok_42") {
        return Err(format!("child env missing in stderr: {stderr_text:?}"));
    }

    unsafe {
        env::remove_var("EXO_COMP_ENV");
    }
    if env::var("EXO_COMP_ENV").is_ok() {
        return Err("env::remove_var failed".into());
    }

    // Non-zero exit code propagation
    let st42 = Command::new(SELF_BIN)
        .args(["--helper", "exit42"])
        .status()
        .map_err(|e| format!("exit42 spawn: {e}"))?;
    if st42.code() != Some(42) {
        return Err(format!("expected exit 42, got {st42}"));
    }

    // Instant & SystemTime & sleep
    let t0 = Instant::now();
    thread::sleep(Duration::from_millis(20));
    let elapsed = t0.elapsed();
    if elapsed < Duration::from_millis(10) {
        return Err(format!("Instant elapsed too short: {elapsed:?}"));
    }
    let _ = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("SystemTime before epoch: {e}"))?;

    // getrandom entropy
    let mut r1 = [0u8; 32];
    let mut r2 = [0u8; 32];
    let n1 = unsafe { syscall3(SYS_GETRANDOM, r1.as_mut_ptr() as u64, 32, 0) };
    let n2 = unsafe { syscall3(SYS_GETRANDOM, r2.as_mut_ptr() as u64, 32, 0) };
    if n1 != 32 || n2 != 32 || r1 == [0u8; 32] || r1 == r2 {
        return Err(format!("getrandom invalid: n1={n1} n2={n2}"));
    }

    // Alloc stress
    let mut map = BTreeMap::new();
    for i in 0u32..512 {
        map.insert(i, vec![(i & 0xff) as u8; 128]);
    }
    if map.len() != 512 || map[&511][0] != 255 {
        return Err("BTreeMap alloc check failed".into());
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// T2: Real OS threads (>64), detached stress, TLS Drop, sync primitives, futex
// ─────────────────────────────────────────────────────────────────────────────

static TLS_DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

struct TlsDropGuard(u64);
impl Drop for TlsDropGuard {
    fn drop(&mut self) {
        if self.0 != 0 {
            TLS_DROP_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }
}

thread_local! {
    static COMP_TLS: std::cell::RefCell<TlsDropGuard> = const { std::cell::RefCell::new(TlsDropGuard(0)) };
}

fn t2_threads_tls_sync() -> Result<(), String> {
    TLS_DROP_COUNT.store(0, Ordering::SeqCst);
    let main_tid = unsafe { syscall0(SYS_GETTID) };

    // 1. Spawn & join 100 threads (5 batches of 20) -> proves >64 OS threads + TLS Drop
    let mut unique_tids = BTreeSet::new();
    for batch in 0..5usize {
        let barrier = Arc::new(Barrier::new(20));
        let mut handles = Vec::with_capacity(20);
        for i in 0..20usize {
            let tag = (batch * 20 + i + 1) as u64;
            let b = Arc::clone(&barrier);
            handles.push(thread::spawn(move || -> i64 {
                COMP_TLS.with(|c| c.borrow_mut().0 = tag);
                let tid = unsafe { syscall0(SYS_GETTID) };
                b.wait();
                tid
            }));
        }
        for h in handles {
            let tid = h.join().map_err(|_| "thread join panic".to_string())?;
            if tid <= 0 || tid == main_tid {
                return Err(format!("invalid worker tid={tid} main={main_tid}"));
            }
            unique_tids.insert(tid);
        }
    }
    if unique_tids.len() < 20 {
        return Err(format!("expected >=20 unique TIDs, got {}", unique_tids.len()));
    }
    let joined_drops = TLS_DROP_COUNT.load(Ordering::SeqCst);
    if joined_drops != 100 {
        return Err(format!("expected 100 joined TLS drops, got {joined_drops}"));
    }

    // 2. Detached threads stress (32 detached threads + TLS Drop)
    let done_pair = Arc::new((Mutex::new(0usize), Condvar::new()));
    for i in 0..32usize {
        let dp = Arc::clone(&done_pair);
        thread::spawn(move || {
            COMP_TLS.with(|c| c.borrow_mut().0 = (1000 + i) as u64);
            let (lock, cv) = &*dp;
            let mut g = lock.lock().unwrap();
            *g += 1;
            if *g == 32 {
                cv.notify_one();
            }
        });
    }
    {
        let (lock, cv) = &*done_pair;
        let mut g = lock.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while *g < 32 {
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                return Err(format!("detached threads timed out at {}", *g));
            }
            let (ng, _) = cv.wait_timeout(g, rem).unwrap();
            g = ng;
        }
    }
    // Give detached threads a short window to run TLS destructors on thread exit
    let drop_deadline = Instant::now() + Duration::from_secs(5);
    while TLS_DROP_COUNT.load(Ordering::SeqCst) < 132 && Instant::now() < drop_deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let total_drops = TLS_DROP_COUNT.load(Ordering::SeqCst);
    if total_drops < 132 {
        return Err(format!("expected 132 total TLS drops (100 joined + 32 detached), got {total_drops}"));
    }

    // 3. Mutex, RwLock, Once, Barrier, mpsc
    let barrier = Arc::new(Barrier::new(8));
    let rw = Arc::new(RwLock::new(0u64));
    let once_counter = Arc::new(AtomicUsize::new(0));
    static TEST_ONCE: Once = Once::new();
    let (tx, rx) = mpsc::channel();
    let mut sync_handles = Vec::new();
    for id in 0..8u64 {
        let b = Arc::clone(&barrier);
        let r = Arc::clone(&rw);
        let oc = Arc::clone(&once_counter);
        let t = tx.clone();
        sync_handles.push(thread::spawn(move || {
            TEST_ONCE.call_once(|| {
                oc.fetch_add(1, Ordering::SeqCst);
            });
            b.wait();
            {
                let mut w = r.write().unwrap();
                *w += id + 1;
            }
            b.wait();
            let val = *r.read().unwrap();
            t.send(val).unwrap();
        }));
    }
    drop(tx);
    for h in sync_handles {
        h.join().map_err(|_| "sync worker panic".to_string())?;
    }
    if once_counter.load(Ordering::SeqCst) != 1 {
        return Err("Once executed more than once".into());
    }
    let received: Vec<u64> = rx.iter().collect();
    if received.len() != 8 || received.iter().any(|&v| v != 36) {
        return Err(format!("RwLock/Barrier/mpsc mismatch: {received:?}"));
    }

    // 4. Raw FUTEX_WAIT_BITSET & FUTEX_WAKE_BITSET
    let futex_word = Arc::new(AtomicU32::new(0));
    let ts = Timespec { tv_sec: 0, tv_nsec: 25_000_000 };
    let tmo_rc = unsafe {
        syscall6(
            SYS_FUTEX,
            &*futex_word as *const AtomicU32 as u64,
            (FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG) as u64,
            0,
            &ts as *const Timespec as u64,
            0,
            0x01,
        )
    };
    if tmo_rc != ETIMEDOUT {
        return Err(format!("FUTEX_WAIT_BITSET timeout expected -110, got {tmo_rc}"));
    }

    let fw_waker = Arc::clone(&futex_word);
    let bitset_waker = thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        // Wrong bitset 0x02 should wake 0 waiters
        let wrong = unsafe {
            syscall6(
                SYS_FUTEX,
                &*fw_waker as *const AtomicU32 as u64,
                (FUTEX_WAKE_BITSET | FUTEX_PRIVATE_FLAG) as u64,
                1,
                0,
                0,
                0x02,
            )
        };
        thread::sleep(Duration::from_millis(15));
        fw_waker.store(1, Ordering::Release);
        let right = unsafe {
            syscall6(
                SYS_FUTEX,
                &*fw_waker as *const AtomicU32 as u64,
                (FUTEX_WAKE_BITSET | FUTEX_PRIVATE_FLAG) as u64,
                1,
                0,
                0,
                0x01,
            )
        };
        (wrong, right)
    });
    let wake_rc = unsafe {
        syscall6(
            SYS_FUTEX,
            &*futex_word as *const AtomicU32 as u64,
            (FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG) as u64,
            0,
            0,
            0,
            0x01,
        )
    };
    let (wrong_w, right_w) = bitset_waker.join().map_err(|_| "waker panic".to_string())?;
    if wake_rc != 0 && wake_rc != EAGAIN {
        return Err(format!("FUTEX_WAIT_BITSET unexpected rc={wake_rc}"));
    }
    if wrong_w != 0 || right_w < 0 {
        return Err(format!("FUTEX_WAKE_BITSET wrong={wrong_w} right={right_w}"));
    }

    // 5. Raw FUTEX_CMP_REQUEUE
    let f1 = Arc::new(AtomicU32::new(0));
    let f2 = Arc::new(AtomicU32::new(0));
    let mut rq_handles = Vec::new();
    for _ in 0..2 {
        let a = Arc::clone(&f1);
        rq_handles.push(thread::spawn(move || {
            unsafe {
                let _ = syscall6(
                    SYS_FUTEX,
                    &*a as *const AtomicU32 as u64,
                    (FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG) as u64,
                    0,
                    0,
                    0,
                    0xffff_ffff,
                );
            }
        }));
    }
    thread::sleep(Duration::from_millis(35));
    let rq_rc = unsafe {
        syscall6(
            SYS_FUTEX,
            &*f1 as *const AtomicU32 as u64,
            (FUTEX_CMP_REQUEUE | FUTEX_PRIVATE_FLAG) as u64,
            1, // wake 1
            1, // requeue 1 to f2
            &*f2 as *const AtomicU32 as u64,
            0, // expected value of *f1
        )
    };
    if rq_rc < 1 {
        return Err(format!("FUTEX_CMP_REQUEUE returned {rq_rc}"));
    }
    thread::sleep(Duration::from_millis(15));
    let w2 = unsafe {
        syscall6(
            SYS_FUTEX,
            &*f2 as *const AtomicU32 as u64,
            (FUTEX_WAKE_BITSET | FUTEX_PRIVATE_FLAG) as u64,
            1,
            0,
            0,
            0xffff_ffff,
        )
    };
    if w2 < 0 {
        return Err(format!("FUTEX_WAKE f2 failed: {w2}"));
    }
    for h in rq_handles {
        h.join().map_err(|_| "requeue waiter panic".to_string())?;
    }

    Ok(())
}

static SIG_HITS: AtomicI32 = AtomicI32::new(0);

extern "C" fn usr1_handler(sig: i32) {
    if sig == SIGUSR1 as i32 {
        // Deliberately clobber XMM / floating-point registers inside signal handler
        let mut x: f64 = 1.618_033_988_75;
        let mut y: f64 = -42.125;
        for i in 1..48 {
            x = (x * 1.013 + (i as f64) * 0.5).sqrt();
            y = y * 0.97 + x;
        }
        core::hint::black_box((x, y));
        SIG_HITS.fetch_add(1, Ordering::SeqCst);
    }
}

fn t2_signals_fpu_mask() -> Result<(), String> {
    SIG_HITS.store(0, Ordering::SeqCst);
    let act = KernelSigAction {
        sa_handler: usr1_handler as *const () as usize as u64,
        sa_flags: SA_RESTORER,
        sa_restorer: sigreturn_trampoline as *const () as usize as u64,
        sa_mask: 0,
    };
    let rc = unsafe {
        syscall4(
            SYS_RT_SIGACTION,
            SIGUSR1,
            &act as *const KernelSigAction as u64,
            0,
            8,
        )
    };
    if rc != 0 {
        return Err(format!("rt_sigaction failed: {rc}"));
    }

    // Block SIGUSR1, send SIGUSR1, verify deferred until unblocked
    let mask_bit: u64 = 1u64 << (SIGUSR1 - 1);
    let mut old_mask: u64 = 0;
    let rc = unsafe {
        syscall4(
            SYS_RT_SIGPROCMASK,
            SIG_BLOCK,
            &mask_bit as *const u64 as u64,
            &mut old_mask as *mut u64 as u64,
            8,
        )
    };
    if rc != 0 {
        return Err(format!("sigprocmask block failed: {rc}"));
    }

    let pid = unsafe { syscall0(SYS_GETPID) } as u64;
    let krc = unsafe { syscall2(SYS_KILL, pid, SIGUSR1) };
    if krc != 0 {
        return Err(format!("kill(SIGUSR1) failed: {krc}"));
    }
    thread::sleep(Duration::from_millis(20));
    if SIG_HITS.load(Ordering::SeqCst) != 0 {
        return Err("SIGUSR1 delivered while blocked in sigprocmask".into());
    }

    // Preserve FPU/XMM registers across signal delivery & sigreturn
    let a = core::hint::black_box(3.141_592_653_589_793f64);
    let b = core::hint::black_box(2.718_281_828_459_045f64);
    let prod_before = core::hint::black_box(a * b);

    let rc = unsafe {
        syscall4(
            SYS_RT_SIGPROCMASK,
            SIG_UNBLOCK,
            &mask_bit as *const u64 as u64,
            0,
            8,
        )
    };
    if rc != 0 {
        return Err(format!("sigprocmask unblock failed: {rc}"));
    }
    let prod_after = core::hint::black_box(a * b);

    if SIG_HITS.load(Ordering::SeqCst) != 1 {
        return Err(format!(
            "expected SIGUSR1 delivered once after unblock, got {}",
            SIG_HITS.load(Ordering::SeqCst)
        ));
    }
    if prod_before.to_bits() != prod_after.to_bits() {
        return Err("FPU/XMM state corrupted across sigreturn".into());
    }

    // Verify sigmask restored (SIGUSR1 unmasked)
    let mut cur_mask: u64 = 0;
    let rc = unsafe {
        syscall4(
            SYS_RT_SIGPROCMASK,
            SIG_BLOCK,
            0,
            &mut cur_mask as *mut u64 as u64,
            8,
        )
    };
    if rc != 0 || (cur_mask & mask_bit) != 0 {
        return Err(format!("sigmask not restored after sigreturn: rc={rc} mask={cur_mask:#x}"));
    }
    Ok(())
}

fn t2_guard_page_sigsegv() -> Result<(), String> {
    let base = unsafe {
        syscall6(
            SYS_MMAP,
            0,
            8192,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            u64::MAX,
            0,
        )
    };
    if base < 0 {
        return Err(format!("mmap failed: {base}"));
    }
    let guard = (base as u64) + 4096;
    let mp = unsafe { syscall3(SYS_MPROTECT, guard, 4096, PROT_NONE) };
    if mp != 0 {
        return Err(format!("mprotect(PROT_NONE) failed: {mp}"));
    }
    // First page must remain writable
    unsafe {
        core::ptr::write_volatile(base as *mut u8, 0x5A);
        let _ = core::ptr::read_volatile(base as *const u8);
        // Guard page read MUST trigger hardware #PF -> SIGSEGV
        let v = core::ptr::read_volatile(guard as *const u8);
        core::hint::black_box(v);
    }
    Err("PROT_NONE guard page read did NOT trigger SIGSEGV".into())
}

// ─────────────────────────────────────────────────────────────────────────────
// T3: Honest POSIX FS (ENOENT missing parent, deferred unlink/rename, *at, flock, utimensat, ELOOP, getdents64, memfd)
// ─────────────────────────────────────────────────────────────────────────────

fn t3_fs_honest_posix() -> Result<(), String> {
    let _ = fs::remove_dir_all("/tmp/comp_fs");
    fs::create_dir_all("/tmp/comp_fs").map_err(|e| format!("mkdir: {e}"))?;

    // 1. O_CREAT into nonexistent parent must fail with NotFound
    match OpenOptions::new()
        .create(true)
        .write(true)
        .open("/tmp/comp_fs/no_such_parent/file.txt")
    {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        other => return Err(format!("O_CREAT missing parent expected NotFound, got {other:?}")),
    }

    // 2. Deferred unlink + recreate without aliasing
    let def_path = "/tmp/comp_fs/deferred.txt";
    let mut old_f = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(def_path)
        .map_err(|e| format!("open old: {e}"))?;
    old_f.write_all(b"OLD_DATA_123").map_err(|e| format!("write old: {e}"))?;
    fs::remove_file(def_path).map_err(|e| format!("unlink: {e}"))?;
    if fs::metadata(def_path).is_ok() {
        return Err("unlinked path still visible in stat".into());
    }
    fs::write(def_path, b"NEW_DATA_999").map_err(|e| format!("recreate: {e}"))?;

    old_f.seek(SeekFrom::Start(0)).map_err(|e| format!("seek old: {e}"))?;
    let mut old_buf = String::new();
    old_f.read_to_string(&mut old_buf).map_err(|e| format!("read old: {e}"))?;
    if old_buf != "OLD_DATA_123" {
        return Err(format!("deferred unlink corrupted old fd: {old_buf:?}"));
    }
    drop(old_f);
    let new_buf = fs::read_to_string(def_path).map_err(|e| format!("read new: {e}"))?;
    if new_buf != "NEW_DATA_999" {
        return Err(format!("recreated file mismatch: {new_buf:?}"));
    }

    // 3. Deferred rename over open FD
    let ren_dst = "/tmp/comp_fs/ren_dst.txt";
    let ren_src = "/tmp/comp_fs/ren_src.txt";
    fs::write(ren_dst, b"ORIG_DST").map_err(|e| format!("write dst: {e}"))?;
    let mut open_dst = File::open(ren_dst).map_err(|e| format!("open dst: {e}"))?;
    fs::write(ren_src, b"REPL_SRC").map_err(|e| format!("write src: {e}"))?;
    fs::rename(ren_src, ren_dst).map_err(|e| format!("rename: {e}"))?;
    let mut orig_read = String::new();
    open_dst.read_to_string(&mut orig_read).map_err(|e| format!("read orig: {e}"))?;
    if orig_read != "ORIG_DST" {
        return Err(format!("deferred rename altered open fd: {orig_read:?}"));
    }
    drop(open_dst);
    if fs::read_to_string(ren_dst).map_err(|e| format!("read replaced: {e}"))? != "REPL_SRC" {
        return Err("rename destination content mismatch".into());
    }

    // 4. *at(dirfd) relative + ENOTDIR on file fd
    let at_dir = "/tmp/comp_fs/atdir";
    fs::create_dir_all(at_dir).map_err(|e| format!("mkdir atdir: {e}"))?;
    let dir_c = b"/tmp/comp_fs/atdir\0";
    let dir_fd = unsafe { syscall3(SYS_OPEN, dir_c.as_ptr() as u64, O_RDONLY, 0) };
    if dir_fd < 0 {
        return Err(format!("open dir failed: {dir_fd}"));
    }
    let rel = b"sub.txt\0";
    let sub_fd = unsafe {
        syscall4(
            SYS_OPENAT,
            dir_fd as u64,
            rel.as_ptr() as u64,
            O_CREAT | O_RDWR | O_TRUNC,
            0o644,
        )
    };
    if sub_fd < 0 {
        return Err(format!("openat relative failed: {sub_fd}"));
    }
    let _ = unsafe { syscall3(SYS_WRITE, sub_fd as u64, b"at_ok".as_ptr() as u64, 5) };
    let mut st = LinuxStat::default();
    let fstat_rc = unsafe {
        syscall4(
            SYS_NEWFSTATAT,
            dir_fd as u64,
            rel.as_ptr() as u64,
            &mut st as *mut LinuxStat as u64,
            0,
        )
    };
    if fstat_rc != 0 || st.st_size != 5 {
        return Err(format!("newfstatat mismatch: rc={fstat_rc} size={}", st.st_size));
    }
    // Non-dir fd passed to openat must return ENOTDIR
    let bad_at = unsafe {
        syscall4(
            SYS_OPENAT,
            sub_fd as u64,
            b"x\0".as_ptr() as u64,
            O_RDONLY,
            0,
        )
    };
    if bad_at != ENOTDIR {
        return Err(format!("openat(file_fd) expected ENOTDIR (-20), got {bad_at}"));
    }

    // 5. fcntl F_GETFL/F_SETFL O_NONBLOCK + unsupported ioctl -> ENOTTY
    let fl0 = unsafe { syscall3(SYS_FCNTL, sub_fd as u64, F_GETFL, 0) };
    if fl0 < 0 {
        return Err(format!("F_GETFL failed: {fl0}"));
    }
    let set_rc = unsafe { syscall3(SYS_FCNTL, sub_fd as u64, F_SETFL, (fl0 as u64) | O_NONBLOCK) };
    let fl1 = unsafe { syscall3(SYS_FCNTL, sub_fd as u64, F_GETFL, 0) };
    if set_rc != 0 || ((fl1 as u64) & O_NONBLOCK) == 0 {
        return Err(format!("O_NONBLOCK did not persist: set={set_rc} fl1={fl1:#x}"));
    }
    let ioctl_rc = unsafe { syscall3(SYS_IOCTL, sub_fd as u64, 0x5401, 0) };
    if ioctl_rc != ENOTTY {
        return Err(format!("unsupported ioctl expected ENOTTY (-25), got {ioctl_rc}"));
    }
    let _ = unsafe { syscall1(SYS_CLOSE, sub_fd as u64) };
    let unl_rc = unsafe { syscall3(SYS_UNLINKAT, dir_fd as u64, rel.as_ptr() as u64, 0) };
    let _ = unsafe { syscall1(SYS_CLOSE, dir_fd as u64) };
    if unl_rc != 0 {
        return Err(format!("unlinkat failed: {unl_rc}"));
    }

    // 6. utimensat timestamp update + readback
    let ut_path = "/tmp/comp_fs/utime.txt";
    fs::write(ut_path, b"u").map_err(|e| format!("write utime: {e}"))?;
    let ut_c = b"/tmp/comp_fs/utime.txt\0";
    let times = [
        Timespec { tv_sec: 1_700_000_100, tv_nsec: 0 },
        Timespec { tv_sec: 1_700_000_123, tv_nsec: 0 },
    ];
    let ut_rc = unsafe {
        syscall4(
            SYS_UTIMENSAT,
            AT_FDCWD as i64 as u64,
            ut_c.as_ptr() as u64,
            times.as_ptr() as u64,
            0,
        )
    };
    let mut ut_st = LinuxStat::default();
    let st_rc = unsafe { syscall2(SYS_STAT, ut_c.as_ptr() as u64, &mut ut_st as *mut LinuxStat as u64) };
    if ut_rc != 0 || st_rc != 0 || ut_st.st_mtime != 1_700_000_123 {
        return Err(format!("utimensat mismatch: ut={ut_rc} st={st_rc} mtime={}", ut_st.st_mtime));
    }

    // 7. Symlink loop -> ELOOP
    let la = b"/tmp/comp_fs/loop_a\0";
    let lb = b"/tmp/comp_fs/loop_b\0";
    let _ = unsafe { syscall2(SYS_SYMLINK, lb.as_ptr() as u64, la.as_ptr() as u64) };
    let _ = unsafe { syscall2(SYS_SYMLINK, la.as_ptr() as u64, lb.as_ptr() as u64) };
    let loop_rc = unsafe { syscall3(SYS_OPEN, la.as_ptr() as u64, O_RDONLY, 0) };
    if loop_rc != ELOOP {
        return Err(format!("symlink loop expected ELOOP (-40), got {loop_rc}"));
    }

    // 8. getdents64 via fs::read_dir
    let list_dir = "/tmp/comp_fs/readdir";
    fs::create_dir_all(list_dir).map_err(|e| format!("mkdir readdir: {e}"))?;
    for i in 0..10 {
        fs::write(format!("{list_dir}/f{i}.txt"), b"x").map_err(|e| format!("write f{i}: {e}"))?;
    }
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(list_dir).map_err(|e| format!("read_dir: {e}"))? {
        let e = entry.map_err(|e| format!("dir entry: {e}"))?;
        names.insert(e.file_name().to_string_lossy().into_owned());
    }
    for i in 0..10 {
        let want = format!("f{i}.txt");
        if !names.contains(&want) {
            return Err(format!("read_dir missing {want} in {names:?}"));
        }
    }

    // 9. memfd_create + write + ftruncate + read
    let mfd = unsafe { syscall2(SYS_MEMFD_CREATE, b"comp_mfd\0".as_ptr() as u64, 0) };
    if mfd < 0 {
        return Err(format!("memfd_create failed: {mfd}"));
    }
    let data64 = [0x5Au8; 64];
    let wn = unsafe { syscall3(SYS_WRITE, mfd as u64, data64.as_ptr() as u64, 64) };
    let tr = unsafe { syscall2(SYS_FTRUNCATE, mfd as u64, 32) };
    let sk = unsafe { syscall3(SYS_LSEEK, mfd as u64, 0, 0) };
    let mut rbuf = [0u8; 64];
    let rn = unsafe { syscall3(SYS_READ, mfd as u64, rbuf.as_mut_ptr() as u64, 64) };
    let _ = unsafe { syscall1(SYS_CLOSE, mfd as u64) };
    if wn != 64 || tr != 0 || sk != 0 || rn != 32 || rbuf[..32] != [0x5Au8; 32] {
        return Err(format!("memfd mismatch: wn={wn} tr={tr} sk={sk} rn={rn}"));
    }

    let _ = fs::remove_dir_all("/tmp/comp_fs");
    Ok(())
}

fn t3_flock_multiprocess() -> Result<(), String> {
    let lock_path = "/tmp/comp_flock.lock";
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(lock_path)
        .map_err(|e| format!("open lock: {e}"))?;
    let fd = file.as_raw_fd() as u64;
    let rc = unsafe { syscall2(SYS_FLOCK, fd, LOCK_EX) };
    if rc != 0 {
        return Err(format!("parent flock(LOCK_EX) failed: {rc}"));
    }

    let mut child = Command::new(SELF_BIN)
        .args(["--helper", "flock-child", lock_path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn flock-child: {e}"))?;
    let mut out = child.stdout.take().ok_or("no child stdout")?;
    let mut line = [0u8; 32];
    let mut got = 0usize;
    while got < 9 {
        let n = out.read(&mut line[got..]).map_err(|e| format!("read ready: {e}"))?;
        if n == 0 {
            break;
        }
        got += n;
    }
    if !line[..got].starts_with(b"BLOCKING") {
        return Err(format!(
            "flock-child did not signal BLOCKING: {:?}",
            String::from_utf8_lossy(&line[..got])
        ));
    }

    thread::sleep(Duration::from_millis(120));
    if let Ok(Some(early)) = child.try_wait() {
        return Err(format!("flock-child bypassed LOCK_EX early: {early}"));
    }

    let urc = unsafe { syscall2(SYS_FLOCK, fd, LOCK_UN) };
    if urc != 0 {
        return Err(format!("parent flock(LOCK_UN) failed: {urc}"));
    }
    let st = child.wait().map_err(|e| format!("wait flock-child: {e}"))?;
    let _ = fs::remove_file(lock_path);
    if !st.success() {
        return Err(format!("flock-child exited with {st}"));
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// T4: AF_UNIX (abstract, SOCK_DGRAM, SO_PEERCRED, SCM_RIGHTS), epoll, Wayland
// ─────────────────────────────────────────────────────────────────────────────

fn t4_unix_dgram_scm_rights() -> Result<(), String> {
    // 1. Abstract namespace SOCK_STREAM + listen() requirement + SO_PEERCRED
    let srv = unsafe { syscall3(SYS_SOCKET, AF_UNIX as u64, SOCK_STREAM as u64, 0) };
    let cli = unsafe { syscall3(SYS_SOCKET, AF_UNIX as u64, SOCK_STREAM as u64, 0) };
    if srv < 0 || cli < 0 {
        return Err(format!("unix socket failed: srv={srv} cli={cli}"));
    }
    let mut sun = [0u8; 110];
    sun[0..2].copy_from_slice(&(AF_UNIX as u16).to_ne_bytes());
    sun[2] = 0; // abstract namespace
    let name = b"exo_comp_abs_s";
    sun[3..3 + name.len()].copy_from_slice(name);
    let alen = (3 + name.len()) as u64;

    if unsafe { syscall2(SYS_LISTEN, srv as u64, 8) } >= 0 {
        return Err("listen before bind should fail".into());
    }
    if unsafe { syscall3(SYS_BIND, srv as u64, sun.as_ptr() as u64, alen) } != 0 {
        return Err("abstract bind failed".into());
    }
    if unsafe { syscall3(SYS_CONNECT, cli as u64, sun.as_ptr() as u64, alen) } != ECONNREFUSED {
        return Err("connect before listen should return ECONNREFUSED".into());
    }
    if unsafe { syscall2(SYS_LISTEN, srv as u64, 8) } != 0 {
        return Err("abstract listen failed".into());
    }
    if unsafe { syscall3(SYS_CONNECT, cli as u64, sun.as_ptr() as u64, alen) } != 0 {
        return Err("abstract connect failed".into());
    }
    let acc = unsafe { syscall3(SYS_ACCEPT, srv as u64, 0, 0) };
    if acc < 0 {
        return Err(format!("abstract accept failed: {acc}"));
    }

    let mut cred = Ucred::default();
    let mut clen = core::mem::size_of::<Ucred>() as u32;
    let grc = unsafe {
        syscall5(
            SYS_GETSOCKOPT,
            acc as u64,
            SOL_SOCKET as u64,
            SO_PEERCRED as u64,
            &mut cred as *mut Ucred as u64,
            &mut clen as *mut u32 as u64,
        )
    };
    let my_pid = unsafe { syscall0(SYS_GETPID) } as i32;
    if grc != 0 || cred.pid != my_pid {
        return Err(format!("SO_PEERCRED mismatch: rc={grc} got={} want={my_pid}", cred.pid));
    }

    let _ = unsafe { syscall6(SYS_SENDTO, cli as u64, b"ABS_OK".as_ptr() as u64, 6, 0, 0, 0) };
    let mut rx = [0u8; 16];
    let rn = unsafe { syscall6(SYS_RECVFROM, acc as u64, rx.as_mut_ptr() as u64, 16, 0, 0, 0) };
    let _ = unsafe { syscall1(SYS_CLOSE, acc as u64) };
    let _ = unsafe { syscall1(SYS_CLOSE, cli as u64) };
    let _ = unsafe { syscall1(SYS_CLOSE, srv as u64) };
    if rn != 6 || &rx[..6] != b"ABS_OK" {
        return Err(format!("abstract stream echo failed: rn={rn}"));
    }

    // 2. AF_UNIX SOCK_DGRAM unconnected sendto/recvfrom with preserved boundaries
    let ds = unsafe { syscall3(SYS_SOCKET, AF_UNIX as u64, SOCK_DGRAM as u64, 0) };
    let dc = unsafe { syscall3(SYS_SOCKET, AF_UNIX as u64, SOCK_DGRAM as u64, 0) };
    let mut sa_s = [0u8; 110];
    sa_s[0..2].copy_from_slice(&(AF_UNIX as u16).to_ne_bytes());
    sa_s[3..14].copy_from_slice(b"exo_dg_srv1");
    let mut sa_c = [0u8; 110];
    sa_c[0..2].copy_from_slice(&(AF_UNIX as u16).to_ne_bytes());
    sa_c[3..14].copy_from_slice(b"exo_dg_cli1");
    if unsafe { syscall3(SYS_BIND, ds as u64, sa_s.as_ptr() as u64, 14) } != 0
        || unsafe { syscall3(SYS_BIND, dc as u64, sa_c.as_ptr() as u64, 14) } != 0
    {
        return Err("dgram bind failed".into());
    }
    let s1 = unsafe { syscall6(SYS_SENDTO, dc as u64, b"ONE".as_ptr() as u64, 3, 0, sa_s.as_ptr() as u64, 14) };
    let s2 = unsafe { syscall6(SYS_SENDTO, dc as u64, b"TWOTWO".as_ptr() as u64, 6, 0, sa_s.as_ptr() as u64, 14) };
    if s1 != 3 || s2 != 6 {
        return Err(format!("dgram sendto failed: s1={s1} s2={s2}"));
    }
    let mut dbuf = [0u8; 32];
    let mut src_out = [0u8; 110];
    let mut src_len = 110u32;
    let r1 = unsafe {
        syscall6(
            SYS_RECVFROM,
            ds as u64,
            dbuf.as_mut_ptr() as u64,
            32,
            0,
            src_out.as_mut_ptr() as u64,
            &mut src_len as *mut u32 as u64,
        )
    };
    if r1 != 3 || &dbuf[..3] != b"ONE" || src_len != 14 || &src_out[3..14] != b"exo_dg_cli1" {
        return Err(format!("dgram pkt1 mismatch: r1={r1} src_len={src_len}"));
    }
    let r2 = unsafe { syscall6(SYS_RECVFROM, ds as u64, dbuf.as_mut_ptr() as u64, 32, 0, 0, 0) };
    let _ = unsafe { syscall1(SYS_CLOSE, dc as u64) };
    let _ = unsafe { syscall1(SYS_CLOSE, ds as u64) };
    if r2 != 6 || &dbuf[..6] != b"TWOTWO" {
        return Err(format!("dgram pkt2 boundary mismatch: r2={r2}"));
    }

    // 3. SCM_RIGHTS: MSG_PEEK zero FD leak + MSG_CTRUNC preserves remaining FD
    let mut sv = [0i32; 2];
    if unsafe { syscall4(SYS_SOCKETPAIR, AF_UNIX as u64, SOCK_STREAM as u64, 0, sv.as_mut_ptr() as u64) } != 0 {
        return Err("socketpair failed".into());
    }
    let efd1 = unsafe { syscall2(SYS_EVENTFD2, 11, 0) } as i32;
    let efd2 = unsafe { syscall2(SYS_EVENTFD2, 22, 0) } as i32;
    if efd1 < 0 || efd2 < 0 {
        return Err("eventfd2 failed".into());
    }
    let tx_b = *b"XY";
    let tx_iov = Iovec { iov_base: tx_b.as_ptr() as u64, iov_len: 2 };
    let mut cmsg_tx = [0u8; 32];
    cmsg_tx[0..8].copy_from_slice(&24u64.to_ne_bytes());
    cmsg_tx[8..12].copy_from_slice(&SOL_SOCKET.to_ne_bytes());
    cmsg_tx[12..16].copy_from_slice(&SCM_RIGHTS.to_ne_bytes());
    cmsg_tx[16..20].copy_from_slice(&efd1.to_ne_bytes());
    cmsg_tx[20..24].copy_from_slice(&efd2.to_ne_bytes());
    let tx_hdr = Msghdr {
        msg_iov: &tx_iov as *const _ as u64,
        msg_iovlen: 1,
        msg_control: cmsg_tx.as_ptr() as u64,
        msg_controllen: 24,
        ..Default::default()
    };
    if unsafe { syscall3(SYS_SENDMSG, sv[0] as u64, &tx_hdr as *const _ as u64, 0) } != 2 {
        return Err("sendmsg SCM_RIGHTS failed".into());
    }

    // MSG_PEEK must NOT consume/leak FDs
    let mut pbuf = [0u8; 4];
    let mut piov = Iovec { iov_base: pbuf.as_mut_ptr() as u64, iov_len: 4 };
    let mut cmsg_peek = [0u8; 32];
    let mut phdr = Msghdr {
        msg_iov: &mut piov as *mut _ as u64,
        msg_iovlen: 1,
        msg_control: cmsg_peek.as_mut_ptr() as u64,
        msg_controllen: 32,
        ..Default::default()
    };
    let prc = unsafe { syscall3(SYS_RECVMSG, sv[1] as u64, &mut phdr as *mut _ as u64, MSG_PEEK as u64) };
    if prc != 2 || &pbuf[..2] != b"XY" || phdr.msg_controllen != 0 {
        return Err(format!("SCM_RIGHTS MSG_PEEK failed: prc={prc} clen={}", phdr.msg_controllen));
    }

    // Receive with 1-FD control buffer (20B) -> MSG_CTRUNC, delivers efd1, preserves efd2
    let mut r1b = [0u8; 1];
    let mut r1iov = Iovec { iov_base: r1b.as_mut_ptr() as u64, iov_len: 1 };
    let mut cmsg1 = [0u8; 20];
    let mut r1hdr = Msghdr {
        msg_iov: &mut r1iov as *mut _ as u64,
        msg_iovlen: 1,
        msg_control: cmsg1.as_mut_ptr() as u64,
        msg_controllen: 20,
        ..Default::default()
    };
    if unsafe { syscall3(SYS_RECVMSG, sv[1] as u64, &mut r1hdr as *mut _ as u64, 0) } != 1
        || (r1hdr.msg_flags & MSG_CTRUNC) == 0
    {
        return Err(format!("SCM_RIGHTS MSG_CTRUNC missing: flags={:#x}", r1hdr.msg_flags));
    }
    let rx_fd1 = i32::from_ne_bytes([cmsg1[16], cmsg1[17], cmsg1[18], cmsg1[19]]);

    // Second recvmsg gets preserved efd2
    let mut r2b = [0u8; 1];
    let mut r2iov = Iovec { iov_base: r2b.as_mut_ptr() as u64, iov_len: 1 };
    let mut cmsg2 = [0u8; 32];
    let mut r2hdr = Msghdr {
        msg_iov: &mut r2iov as *mut _ as u64,
        msg_iovlen: 1,
        msg_control: cmsg2.as_mut_ptr() as u64,
        msg_controllen: 32,
        ..Default::default()
    };
    if unsafe { syscall3(SYS_RECVMSG, sv[1] as u64, &mut r2hdr as *mut _ as u64, 0) } != 1
        || (r2hdr.msg_flags & MSG_CTRUNC) != 0
    {
        return Err("second recvmsg failed or still truncated".into());
    }
    let rx_fd2 = i32::from_ne_bytes([cmsg2[16], cmsg2[17], cmsg2[18], cmsg2[19]]);

    let mut v1 = 0u64;
    let mut v2 = 0u64;
    let n1 = unsafe { syscall3(SYS_READ, rx_fd1 as u64, &mut v1 as *mut u64 as u64, 8) };
    let n2 = unsafe { syscall3(SYS_READ, rx_fd2 as u64, &mut v2 as *mut u64 as u64, 8) };
    for fd in [rx_fd1, rx_fd2, efd1, efd2, sv[0], sv[1]] {
        let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };
    }
    if n1 != 8 || n2 != 8 || v1 != 11 || v2 != 22 {
        return Err(format!("SCM_RIGHTS eventfd values mismatch: v1={v1} v2={v2}"));
    }
    Ok(())
}

fn t4_epoll_event_driven() -> Result<(), String> {
    let mut sv = [0i32; 2];
    if unsafe { syscall4(SYS_SOCKETPAIR, AF_UNIX as u64, SOCK_STREAM as u64, 0, sv.as_mut_ptr() as u64) } != 0 {
        return Err("socketpair failed".into());
    }
    let epfd = unsafe { syscall1(SYS_EPOLL_CREATE1, 0) };
    if epfd < 0 {
        return Err(format!("epoll_create1 failed: {epfd}"));
    }
    let ev = EpollEvent {
        events: EPOLLIN | EPOLLET | EPOLLRDHUP,
        data: 0xDEAD_BEEF_CAFE_1234,
    };
    if unsafe { syscall4(SYS_EPOLL_CTL, epfd as u64, EPOLL_CTL_ADD as u64, sv[1] as u64, &ev as *const _ as u64) } != 0 {
        return Err("epoll_ctl ADD failed".into());
    }

    let mut out = [EpollEvent::default(); 4];
    if unsafe { syscall4(SYS_EPOLL_WAIT, epfd as u64, out.as_mut_ptr() as u64, 4, 0) } != 0 {
        return Err("idle epoll_wait should return 0".into());
    }

    // Event-driven wakeup from another thread
    let wfd = sv[0];
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        let _ = unsafe { syscall3(SYS_WRITE, wfd as u64, b"EDGE".as_ptr() as u64, 4) };
        thread::sleep(Duration::from_millis(80));
        let _ = unsafe { syscall2(SYS_SHUTDOWN, wfd as u64, 1) };
    });

    let n = unsafe { syscall4(SYS_EPOLL_WAIT, epfd as u64, out.as_mut_ptr() as u64, 4, 5000) };
    let ev0_events = out[0].events;
    let ev0_data = out[0].data;
    if n != 1 || (ev0_events & EPOLLIN) == 0 || ev0_data != 0xDEAD_BEEF_CAFE_1234 {
        return Err(format!("EPOLLET wakeup mismatch: n={n} ev={ev0_events:#x} data={ev0_data:#x}"));
    }
    // Without fresh edge, second wait returns 0 even with unread bytes
    if unsafe { syscall4(SYS_EPOLL_WAIT, epfd as u64, out.as_mut_ptr() as u64, 4, 0) } != 0 {
        return Err("EPOLLET re-triggered without fresh edge".into());
    }

    // Wait for peer SHUT_WR -> EPOLLRDHUP
    let n2 = unsafe { syscall4(SYS_EPOLL_WAIT, epfd as u64, out.as_mut_ptr() as u64, 4, 5000) };
    let ev1_events = out[0].events;
    if n2 != 1 || (ev1_events & EPOLLRDHUP) == 0 {
        return Err(format!("EPOLLRDHUP missing: n2={n2} ev={ev1_events:#x}"));
    }

    // EPOLLONESHOT: fires once, disarmed until MOD
    let ev_one = EpollEvent { events: EPOLLIN | EPOLLONESHOT, data: 77 };
    if unsafe { syscall4(SYS_EPOLL_CTL, epfd as u64, EPOLL_CTL_MOD as u64, sv[1] as u64, &ev_one as *const _ as u64) } != 0 {
        return Err("epoll_ctl MOD ONESHOT failed".into());
    }
    let n3 = unsafe { syscall4(SYS_EPOLL_WAIT, epfd as u64, out.as_mut_ptr() as u64, 4, 0) };
    let n4 = unsafe { syscall4(SYS_EPOLL_WAIT, epfd as u64, out.as_mut_ptr() as u64, 4, 0) };
    let _ = writer.join();
    for fd in [epfd as i32, sv[0], sv[1]] {
        let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };
    }
    if n3 != 1 || n4 != 0 {
        return Err(format!("EPOLLONESHOT failed: first={n3} second={n4}"));
    }
    Ok(())
}

fn t4_wayland_live_ipc() -> Result<(), String> {
    let fd = unsafe { syscall3(SYS_SOCKET, AF_UNIX as u64, SOCK_STREAM as u64, 0) };
    if fd < 0 {
        return Err(format!("wayland unix socket failed: {fd}"));
    }
    let mut sun = [0u8; 110];
    sun[0..2].copy_from_slice(&(AF_UNIX as u16).to_ne_bytes());
    let path = b"/run/wayland-0";
    sun[2..2 + path.len()].copy_from_slice(path);
    let alen = (2 + path.len() + 1) as u64;
    let crc = unsafe { syscall3(SYS_CONNECT, fd as u64, sun.as_ptr() as u64, alen) };
    if crc != 0 {
        let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };
        return Err(format!("connect /run/wayland-0 failed: {crc}"));
    }

    // Wayland wire requests:
    // 1. wl_display(1)::get_registry(opcode=1, size=12) new_id=2
    // 2. wl_display(1)::sync(opcode=0, size=12) new_id=3
    let mut req = [0u8; 24];
    req[0..4].copy_from_slice(&1u32.to_ne_bytes());
    req[4..8].copy_from_slice(&((12u32 << 16) | 1u32).to_ne_bytes());
    req[8..12].copy_from_slice(&2u32.to_ne_bytes());
    req[12..16].copy_from_slice(&1u32.to_ne_bytes());
    req[16..20].copy_from_slice(&((12u32 << 16) | 0u32).to_ne_bytes());
    req[20..24].copy_from_slice(&3u32.to_ne_bytes());

    let wn = unsafe { syscall3(SYS_WRITE, fd as u64, req.as_ptr() as u64, req.len() as u64) };
    if wn != 24 {
        let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };
        return Err(format!("wayland write failed: {wn}"));
    }

    let epfd = unsafe { syscall1(SYS_EPOLL_CREATE1, 0) };
    let ev = EpollEvent { events: EPOLLIN, data: 1 };
    let _ = unsafe { syscall4(SYS_EPOLL_CTL, epfd as u64, EPOLL_CTL_ADD as u64, fd as u64, &ev as *const _ as u64) };

    let mut wire = Vec::new();
    let mut globals = 0usize;
    let mut sync_done = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ev_out = [EpollEvent::default(); 2];
    let mut tmp = [0u8; 1024];

    while !sync_done && Instant::now() < deadline {
        let ready = unsafe { syscall4(SYS_EPOLL_WAIT, epfd as u64, ev_out.as_mut_ptr() as u64, 2, 500) };
        if ready <= 0 {
            continue;
        }
        let n = unsafe { syscall3(SYS_READ, fd as u64, tmp.as_mut_ptr() as u64, tmp.len() as u64) };
        if n <= 0 {
            break;
        }
        wire.extend_from_slice(&tmp[..n as usize]);
        let mut pos = 0usize;
        while pos + 8 <= wire.len() {
            let obj = u32::from_ne_bytes([wire[pos], wire[pos + 1], wire[pos + 2], wire[pos + 3]]);
            let word = u32::from_ne_bytes([wire[pos + 4], wire[pos + 5], wire[pos + 6], wire[pos + 7]]);
            let opcode = word & 0xffff;
            let size = (word >> 16) as usize;
            if size < 8 || pos + size > wire.len() {
                break;
            }
            if obj == 2 && opcode == 0 {
                globals += 1;
            } else if obj == 3 && opcode == 0 {
                sync_done = true;
            }
            pos += size;
        }
        if pos > 0 {
            wire.drain(..pos);
        }
    }

    let _ = unsafe { syscall1(SYS_CLOSE, epfd as u64) };
    let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };

    if !sync_done || globals < 2 {
        return Err(format!("wayland roundtrip incomplete: sync_done={sync_done} globals={globals}"));
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// T5: Loopback TCP (>1 MiB, concurrent accept, epoll, half-close, EINPROGRESS/SO_ERROR),
//     UDP (>512B), RFC 1035 DNS wire client + /etc/hosts + hostname
// ─────────────────────────────────────────────────────────────────────────────

fn t5_tcp_loopback_1mb_and_epoll() -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind: {e}"))?;
    let addr = listener.local_addr().map_err(|e| format!("local_addr: {e}"))?;
    if addr.port() == 0 {
        return Err("listener ephemeral port was 0".into());
    }

    // Server handles:
    // - 3 concurrent echo connections (2 x 4KiB + 1 x 1MiB)
    // - 1 epoll + half-close test connection
    // - 1 nonblocking connect test connection
    let srv_handle = thread::spawn(move || -> Result<(), String> {
        // Phase A: 3 concurrent echo workers
        let mut workers = Vec::new();
        for _ in 0..3 {
            let (mut stream, _peer) = listener.accept().map_err(|e| format!("accept echo: {e}"))?;
            workers.push(thread::spawn(move || -> Result<(), String> {
                let mut buf = [0u8; 16384];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => stream.write_all(&buf[..n]).map_err(|e| format!("srv write: {e}"))?,
                        Err(e) => return Err(format!("srv read: {e}")),
                    }
                }
                let _ = stream.shutdown(Shutdown::Write);
                Ok(())
            }));
        }
        for w in workers {
            w.join().map_err(|_| "echo worker panic".to_string())??;
        }

        // Phase B: epoll on TcpStream + half-close FIN -> clean EOF -> reply still works
        let (mut hc_stream, _) = listener.accept().map_err(|e| format!("accept hc: {e}"))?;
        let epfd = unsafe { syscall1(SYS_EPOLL_CREATE1, 0) };
        if epfd < 0 {
            return Err("epoll_create1 for tcp failed".into());
        }
        let ev = EpollEvent { events: EPOLLIN | EPOLLOUT, data: 0x55 };
        if unsafe { syscall4(SYS_EPOLL_CTL, epfd as u64, EPOLL_CTL_ADD as u64, hc_stream.as_raw_fd() as u64, &ev as *const _ as u64) } != 0 {
            let _ = unsafe { syscall1(SYS_CLOSE, epfd as u64) };
            return Err("epoll_ctl ADD tcp failed".into());
        }
        let mut got = Vec::new();
        let mut tmp = [0u8; 64];
        let mut ev_out = [EpollEvent::default(); 2];
        loop {
            let r = unsafe { syscall4(SYS_EPOLL_WAIT, epfd as u64, ev_out.as_mut_ptr() as u64, 2, 5000) };
            if r <= 0 {
                let _ = unsafe { syscall1(SYS_CLOSE, epfd as u64) };
                return Err(format!("tcp epoll_wait timed out: {r}"));
            }
            if (ev_out[0].events & EPOLLIN) != 0 {
                match hc_stream.read(&mut tmp) {
                    Ok(0) => break, // clean EOF from peer FIN!
                    Ok(n) => got.extend_from_slice(&tmp[..n]),
                    Err(e) => {
                        let _ = unsafe { syscall1(SYS_CLOSE, epfd as u64) };
                        return Err(format!("hc read: {e}"));
                    }
                }
            }
        }
        let _ = unsafe { syscall1(SYS_CLOSE, epfd as u64) };
        if got != b"HALFCLOSE" {
            return Err(format!("half-close payload mismatch: {got:?}"));
        }
        hc_stream.write_all(b"STILL_ALIVE").map_err(|e| format!("hc reply: {e}"))?;
        let _ = hc_stream.shutdown(Shutdown::Write);

        // Phase C: accept nonblocking connect
        let (_nb_stream, _) = listener.accept().map_err(|e| format!("accept nb: {e}"))?;
        Ok(())
    });

    // Client Phase A: 2 concurrent 4 KiB clients + 1 concurrent 1 MiB bidirectional echo client
    let mut cli_handles = Vec::new();
    for cid in 0..2u8 {
        cli_handles.push(thread::spawn(move || -> Result<(), String> {
            let mut s = TcpStream::connect(addr).map_err(|e| format!("cli{cid} connect: {e}"))?;
            let data: Vec<u8> = (0..4096usize).map(|i| (i as u8).wrapping_add(cid * 71)).collect();
            let mut w = s.try_clone().map_err(|e| format!("clone: {e}"))?;
            let tx = data.clone();
            let wt = thread::spawn(move || -> io::Result<()> {
                w.write_all(&tx)?;
                w.shutdown(Shutdown::Write)?;
                Ok(())
            });
            let mut rx = Vec::with_capacity(4096);
            s.read_to_end(&mut rx).map_err(|e| format!("cli{cid} read: {e}"))?;
            wt.join().map_err(|_| "wt panic".to_string())?.map_err(|e| format!("wt err: {e}"))?;
            if rx != data {
                return Err(format!("cli{cid} echo mismatch len={}", rx.len()));
            }
            Ok(())
        }));
    }

    // 1 MiB (1,048,576 bytes) concurrent TCP stream echo!
    cli_handles.push(thread::spawn(move || -> Result<(), String> {
        const ONE_MIB: usize = 1024 * 1024;
        let mut s = TcpStream::connect(addr).map_err(|e| format!("1MiB connect: {e}"))?;
        let mut w = s.try_clone().map_err(|e| format!("1MiB clone: {e}"))?;
        let wt = thread::spawn(move || -> io::Result<()> {
            let mut chunk = [0u8; 16384];
            let mut sent = 0usize;
            while sent < ONE_MIB {
                for (i, b) in chunk.iter_mut().enumerate() {
                    *b = (((sent + i) * 13 + 5) & 0xff) as u8;
                }
                w.write_all(&chunk)?;
                sent += chunk.len();
            }
            w.shutdown(Shutdown::Write)?;
            Ok(())
        });
        let mut total = 0usize;
        let mut buf = [0u8; 16384];
        loop {
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    for i in 0..n {
                        let expect = (((total + i) * 13 + 5) & 0xff) as u8;
                        if buf[i] != expect {
                            return Err(format!("1MiB corruption at offset {}", total + i));
                        }
                    }
                    total += n;
                }
                Err(e) => return Err(format!("1MiB read at {total}: {e}")),
            }
        }
        wt.join().map_err(|_| "1MiB writer panic".to_string())?.map_err(|e| format!("1MiB write: {e}"))?;
        if total != ONE_MIB {
            return Err(format!("1MiB short echo: got {total}, expected {ONE_MIB}"));
        }
        Ok(())
    }));

    for ch in cli_handles {
        ch.join().map_err(|_| "cli panic".to_string())??;
    }

    // Client Phase B: Half-close FIN -> still receives server reply
    {
        let mut hc = TcpStream::connect(addr).map_err(|e| format!("hc connect: {e}"))?;
        hc.write_all(b"HALFCLOSE").map_err(|e| format!("hc write: {e}"))?;
        hc.shutdown(Shutdown::Write).map_err(|e| format!("hc shutdown: {e}"))?;
        let mut reply = Vec::new();
        hc.read_to_end(&mut reply).map_err(|e| format!("hc read reply: {e}"))?;
        if reply != b"STILL_ALIVE" {
            return Err(format!("hc reply mismatch: {reply:?}"));
        }
    }

    // Client Phase C: O_NONBLOCK connect -> EINPROGRESS + epoll EPOLLOUT + SO_ERROR == 0
    {
        let fd = unsafe {
            syscall3(
                SYS_SOCKET,
                AF_INET as u64,
                (SOCK_STREAM | SOCK_NONBLOCK) as u64,
                0,
            )
        };
        if fd < 0 {
            return Err(format!("nb socket failed: {fd}"));
        }
        let raw_addr = SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: addr.port().to_be(),
            sin_addr: [127, 0, 0, 1],
            sin_zero: [0; 8],
        };
        let crc = unsafe {
            syscall3(
                SYS_CONNECT,
                fd as u64,
                &raw_addr as *const SockAddrIn as u64,
                core::mem::size_of::<SockAddrIn>() as u64,
            )
        };
        if crc != 0 && crc != EINPROGRESS {
            let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };
            return Err(format!("nb connect expected 0 or EINPROGRESS, got {crc}"));
        }
        let epfd = unsafe { syscall1(SYS_EPOLL_CREATE1, 0) };
        let ev = EpollEvent { events: EPOLLOUT, data: 1 };
        let _ = unsafe { syscall4(SYS_EPOLL_CTL, epfd as u64, EPOLL_CTL_ADD as u64, fd as u64, &ev as *const _ as u64) };
        let mut ev_out = [EpollEvent::default(); 2];
        let er = unsafe { syscall4(SYS_EPOLL_WAIT, epfd as u64, ev_out.as_mut_ptr() as u64, 2, 5000) };
        let mut so_err: i32 = -1;
        let mut slen: u32 = 4;
        let grc = unsafe {
            syscall5(
                SYS_GETSOCKOPT,
                fd as u64,
                SOL_SOCKET as u64,
                SO_ERROR as u64,
                &mut so_err as *mut i32 as u64,
                &mut slen as *mut u32 as u64,
            )
        };
        let _ = unsafe { syscall1(SYS_CLOSE, epfd as u64) };
        let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };
        if er <= 0 || grc != 0 || so_err != 0 {
            return Err(format!("nb connect completion failed: er={er} grc={grc} so_err={so_err}"));
        }
    }

    srv_handle.join().map_err(|_| "tcp server panic".to_string())??;

    // Client Phase D: Connect to closed loopback port -> ECONNREFUSED
    let closed_port = {
        let tmp = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("tmp bind: {e}"))?;
        tmp.local_addr().map_err(|e| format!("tmp addr: {e}"))?.port()
    };
    let closed_sa = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, closed_port));
    match TcpStream::connect_timeout(&closed_sa, Duration::from_secs(2)) {
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {}
        Err(e) => {
            // Accept ConnectionRefused (or raw errno 111)
            if e.raw_os_error() != Some(111) {
                return Err(format!("closed port expected ConnectionRefused, got {e:?}"));
            }
        }
        Ok(_) => return Err("connect to closed port unexpectedly succeeded".into()),
    }

    Ok(())
}

fn t5_udp_large_dns_hostname() -> Result<(), String> {
    // 1. UDP loopback echo > 512 B (1400 bytes)
    let srv = UdpSocket::bind("127.0.0.1:0").map_err(|e| format!("udp srv bind: {e}"))?;
    let srv_addr = srv.local_addr().map_err(|e| format!("udp srv addr: {e}"))?;
    let cli = UdpSocket::bind("127.0.0.1:0").map_err(|e| format!("udp cli bind: {e}"))?;
    let _ = srv.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = cli.set_read_timeout(Some(Duration::from_secs(3)));

    let big_dgram: Vec<u8> = (0..1400usize).map(|i| ((i * 7 + 3) & 0xff) as u8).collect();
    let expected = big_dgram.clone();
    let st = thread::spawn(move || -> Result<(), String> {
        let mut buf = [0u8; 2048];
        let (n, peer) = srv.recv_from(&mut buf).map_err(|e| format!("udp srv recv: {e}"))?;
        if n != 1400 || buf[..n] != expected[..] {
            return Err(format!("udp srv recv mismatch n={n}"));
        }
        let sn = srv.send_to(&buf[..n], peer).map_err(|e| format!("udp srv send: {e}"))?;
        if sn != 1400 {
            return Err(format!("udp srv send short {sn}"));
        }
        Ok(())
    });

    let sn = cli.send_to(&big_dgram, srv_addr).map_err(|e| format!("udp cli send: {e}"))?;
    if sn != 1400 {
        return Err(format!("udp cli send short {sn}"));
    }
    let mut rx = [0u8; 2048];
    let (rn, from) = cli.recv_from(&mut rx).map_err(|e| format!("udp cli recv: {e}"))?;
    st.join().map_err(|_| "udp srv panic".to_string())??;
    if rn != 1400 || from != srv_addr || rx[..rn] != big_dgram[..] {
        return Err(format!("udp cli echo mismatch rn={rn} from={from}"));
    }

    // 2. DNS localhost + /etc/hosts
    let local: Vec<SocketAddr> = ("localhost", 80)
        .to_socket_addrs()
        .map_err(|e| format!("lookup localhost: {e}"))?
        .collect();
    if !local.contains(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 80))) {
        return Err(format!("localhost resolution mismatch: {local:?}"));
    }

    let orig_hosts = fs::read_to_string("/etc/hosts").ok();
    let _ = fs::create_dir_all("/etc");
    fs::write("/etc/hosts", "127.0.0.1 localhost\n127.0.0.42 exo-comp-hosts.local\n")
        .map_err(|e| format!("write /etc/hosts: {e}"))?;
    let hosts_res = ("exo-comp-hosts.local", 8080)
        .to_socket_addrs()
        .map(|it| it.collect::<Vec<_>>());
    match orig_hosts {
        Some(prev) => {
            let _ = fs::write("/etc/hosts", prev);
        }
        None => {
            let _ = fs::remove_file("/etc/hosts");
        }
    }
    let hosts_addrs = hosts_res.map_err(|e| format!("/etc/hosts lookup failed: {e}"))?;
    if !hosts_addrs.contains(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 42), 8080))) {
        return Err(format!("/etc/hosts addr mismatch: {hosts_addrs:?}"));
    }

    // 3. Live RFC 1035 DNS wire client test against local loopback mock DNS server (127.0.0.1:53)
    let dns_srv = UdpSocket::bind("127.0.0.1:53").map_err(|e| format!("bind 127.0.0.1:53: {e}"))?;
    let _ = dns_srv.set_read_timeout(Some(Duration::from_secs(4)));
    let orig_resolv = fs::read_to_string("/etc/resolv.conf").ok();
    fs::write("/etc/resolv.conf", "nameserver 127.0.0.1\n")
        .map_err(|e| format!("write /etc/resolv.conf: {e}"))?;

    let dns_thread = thread::spawn(move || -> Result<(), String> {
        let mut q = [0u8; 512];
        let (n, peer) = dns_srv.recv_from(&mut q).map_err(|e| format!("mock dns recv: {e}"))?;
        if n < 17 {
            return Err(format!("mock dns short query: {n}"));
        }
        // Build RFC 1035 response copying query header+question and adding pointer-compressed A RR
        let mut resp = Vec::with_capacity(n + 16);
        resp.extend_from_slice(&q[..n]);
        resp[2] = 0x81; // QR=1, RD=1
        resp[3] = 0x80; // RA=1, RCODE=0
        resp[6..8].copy_from_slice(&1u16.to_be_bytes()); // ANCOUNT = 1
        resp.extend_from_slice(&[0xC0, 0x0C]); // Name compression pointer -> offset 12
        resp.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
        resp.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
        resp.extend_from_slice(&60u32.to_be_bytes()); // TTL
        resp.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH = 4
        resp.extend_from_slice(&[10, 99, 88, 77]); // 10.99.88.77
        dns_srv.send_to(&resp, peer).map_err(|e| format!("mock dns send: {e}"))?;
        Ok(())
    });

    let wire_res = ("dns-wire.exo.test", 443)
        .to_socket_addrs()
        .map(|it| it.collect::<Vec<_>>());
    match orig_resolv {
        Some(prev) => {
            let _ = fs::write("/etc/resolv.conf", prev);
        }
        None => {
            let _ = fs::remove_file("/etc/resolv.conf");
        }
    }
    dns_thread.join().map_err(|_| "mock dns panic".to_string())??;
    let wire_addrs = wire_res.map_err(|e| format!("RFC1035 DNS lookup failed: {e}"))?;
    if !wire_addrs.contains(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 99, 88, 77), 443))) {
        return Err(format!("RFC1035 DNS answer mismatch: {wire_addrs:?}"));
    }

    // 4. Hostname via SYS_UNAME
    let mut uts = UtsName::default();
    let urc = unsafe { syscall1(SYS_UNAME, &mut uts as *mut UtsName as u64) };
    let node_len = uts.nodename.iter().position(|&b| b == 0).unwrap_or(uts.nodename.len());
    if urc != 0 || node_len == 0 {
        return Err(format!("uname hostname check failed: rc={urc} node_len={node_len}"));
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// T6: Dynamic loader (/lib/ld-exo.so) & musl-exo verification probes
// ─────────────────────────────────────────────────────────────────────────────

fn run_pie_self_check() -> Result<String, String> {
    use std::cell::Cell;
    thread_local! {
        static PIE_TLS: Cell<u64> = const { Cell::new(0x5049_455F_544C_5331) };
    }
    let code_addr = main as *const () as usize;
    if code_addr < 0x0000_0100_0000_0000 {
        return Err(format!("expected ASLR PIE load bias >= 0x10000000000, got {code_addr:#x}"));
    }
    let main_tls = PIE_TLS.with(|c| {
        let v = c.get();
        c.set(v ^ 0xFF);
        v
    });
    if main_tls != 0x5049_455F_544C_5331 {
        return Err(format!("main PIE TLS init mismatch: {main_tls:#x}"));
    }
    let worker_tls = thread::spawn(|| PIE_TLS.with(|c| c.get()))
        .join()
        .map_err(|_| "PIE worker thread panic".to_string())?;
    if worker_tls != 0x5049_455F_544C_5331 {
        return Err(format!("worker PIE TLS init mismatch: {worker_tls:#x}"));
    }
    let bt = std::backtrace::Backtrace::force_capture();
    let bt_text = format!("{bt:#}");
    let bt_frames = bt_text.lines().filter(|l| l.contains("0x")).count();
    if bt.status() != std::backtrace::BacktraceStatus::Captured || bt_frames < 3 {
        return Err(format!(
            "PIE Backtrace capture failed: status={:?} frames={bt_frames} text={bt_text:?}",
            bt.status()
        ));
    }
    Ok(format!("[EXO_PIE] ALL_PASS addr={code_addr:#x} bt_frames={bt_frames}"))
}

fn t6_dynamic_loader_and_musl() -> Result<(), String> {
    // 1. std::backtrace::Backtrace::force_capture() via frame-pointer _Unwind_Backtrace
    let bt = std::backtrace::Backtrace::force_capture();
    if bt.status() != std::backtrace::BacktraceStatus::Captured {
        return Err(format!("Backtrace::force_capture status={:?}", bt.status()));
    }
    let bt_text = format!("{bt:#}");
    let bt_frames = bt_text.lines().filter(|l| l.contains("0x")).count();
    if bt_frames < 4 {
        return Err(format!(
            "Backtrace expected >=4 captured 0x frames, got {bt_frames}: {bt_text:?}"
        ));
    }

    // 2. Live ASLR Rust std static-PIE executable (x86_64-unknown-exo-pie.json)
    let pie_out = Command::new("/bin/exo-compliance-pie")
        .arg("--pie-self-check")
        .output()
        .map_err(|e| format!("exec /bin/exo-compliance-pie: {e}"))?;
    let pie_stdout = String::from_utf8_lossy(&pie_out.stdout);
    let pie_stderr = String::from_utf8_lossy(&pie_out.stderr);
    if !pie_out.status.success() || !pie_stdout.contains("[EXO_PIE] ALL_PASS") {
        return Err(format!(
            "exo-compliance-pie failed ({}): stdout={} stderr={}",
            pie_out.status,
            pie_stdout.trim(),
            pie_stderr.trim()
        ));
    }

    let dyn_out = Command::new("/bin/p3dyn")
        .output()
        .map_err(|e| format!("exec /bin/p3dyn: {e}"))?;
    let dyn_stdout = String::from_utf8_lossy(&dyn_out.stdout);
    if !dyn_out.status.success() || !dyn_stdout.contains("[P3_PROBE] ALL_PASS") {
        return Err(format!(
            "p3dyn failed ({}): {}",
            dyn_out.status,
            dyn_stdout.trim()
        ));
    }

    let musl_out = Command::new("/bin/p3musl")
        .output()
        .map_err(|e| format!("exec /bin/p3musl: {e}"))?;
    let musl_stdout = String::from_utf8_lossy(&musl_out.stdout);
    if !musl_out.status.success() || !musl_stdout.contains("[P3_MUSL] ALL_PASS") {
        return Err(format!(
            "p3musl failed ({}): {}",
            musl_out.status,
            musl_stdout.trim()
        ));
    }
    Ok(())
}
