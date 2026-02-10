//! Tests complets pour exo_std en conditions réelles
//!
//! Tests exécutés dans le kernel Exo-OS avec hardware réel

use alloc::string::String;
use alloc::format;
use alloc::vec::Vec;

/// Helper pour afficher des nombres
fn print_u64(n: u64) {
    let s = format!("{}", n);
    crate::logger::early_print(&s);
}

fn print_bool(b: bool) {
    if b {
        crate::logger::early_print("PASS");
    } else {
        crate::logger::early_print("FAIL");
    }
}

/// Test 1: HashMap - Insertion, lookup, remove
pub fn test_hashmap() {
    crate::logger::early_print("\n=== TEST 1: HashMap ===\n");

    use exo_std::collections::HashMap;

    let mut map = HashMap::new();

    // Test insertion
    crate::logger::early_print("[1.1] Insert 100 items... ");
    for i in 0..100 {
        map.insert(i, i * 2);
    }
    crate::logger::early_print("OK\n");

    // Test lookup
    crate::logger::early_print("[1.2] Lookup items... ");
    let mut all_found = true;
    for i in 0..100 {
        if map.get(&i) != Some(&(i * 2)) {
            all_found = false;
            break;
        }
    }
    print_bool(all_found);
    crate::logger::early_print("\n");

    // Test length
    crate::logger::early_print("[1.3] Check length = 100... ");
    print_bool(map.len() == 100);
    crate::logger::early_print("\n");

    // Test remove
    crate::logger::early_print("[1.4] Remove item... ");
    let removed = map.remove(&50);
    print_bool(removed == Some(100) && map.len() == 99);
    crate::logger::early_print("\n");

    crate::logger::early_print("HashMap tests: COMPLETE\n");
}

/// Test 2: BTreeMap - Ordered operations
pub fn test_btreemap() {
    crate::logger::early_print("\n=== TEST 2: BTreeMap ===\n");

    use exo_std::collections::BTreeMap;

    let mut tree = BTreeMap::new();

    // Test insertion
    crate::logger::early_print("[2.1] Insert 50 items... ");
    for i in 0..50 {
        tree.insert(i, i * 3);
    }
    crate::logger::early_print("OK\n");

    // Test lookup
    crate::logger::early_print("[2.2] Lookup items... ");
    let mut all_found = true;
    for i in 0..50 {
        if tree.get(&i) != Some(&(i * 3)) {
            all_found = false;
            break;
        }
    }
    print_bool(all_found);
    crate::logger::early_print("\n");

    // Test iteration order
    crate::logger::early_print("[2.3] Check iteration order... ");
    let items: Vec<_> = tree.iter().map(|(k, _)| *k).collect();
    let mut is_sorted = true;
    for i in 1..items.len() {
        if items[i] <= items[i-1] {
            is_sorted = false;
            break;
        }
    }
    print_bool(is_sorted);
    crate::logger::early_print("\n");

    crate::logger::early_print("BTreeMap tests: COMPLETE\n");
}

/// Test 3: Futex - Lock/unlock performance
pub fn test_futex() {
    crate::logger::early_print("\n=== TEST 3: Futex ===\n");

    use exo_std::sync::FutexMutex;

    let mutex = FutexMutex::new();

    // Test lock/unlock
    crate::logger::early_print("[3.1] Lock/unlock 1000 times... ");
    for _ in 0..1000 {
        mutex.lock();
        mutex.unlock();
    }
    crate::logger::early_print("OK\n");

    // Test semaphore
    crate::logger::early_print("[3.2] FutexSemaphore... ");
    use exo_std::sync::FutexSemaphore;
    let sem = FutexSemaphore::new(1);

    let acquired = sem.try_acquire();
    print_bool(acquired);
    crate::logger::early_print(" ");

    sem.release();
    crate::logger::early_print("OK\n");

    crate::logger::early_print("Futex tests: COMPLETE\n");
}

/// Test 4: TLS - Thread-Local Storage
pub fn test_tls() {
    crate::logger::early_print("\n=== TEST 4: TLS ===\n");

    use exo_std::thread::tls::{TlsTemplate, TlsBlock};

    // Create a test TLS template
    static TLS_DATA: [u8; 64] = [42u8; 64];

    let template = TlsTemplate::new(
        TLS_DATA.as_ptr() as usize,
        64,
        128,
        16,
    );

    crate::logger::early_print("[4.1] Template validity... ");
    print_bool(template.is_valid());
    crate::logger::early_print("\n");

    crate::logger::early_print("[4.2] Allocate TLS block... ");
    unsafe {
        match TlsBlock::allocate(&template) {
            Ok(block) => {
                crate::logger::early_print("OK (");
                print_u64(block.size() as u64);
                crate::logger::early_print(" bytes)\n");

                // Verify data
                crate::logger::early_print("[4.3] Verify initialized data... ");
                let data = core::slice::from_raw_parts(block.as_ptr(), 64);
                let all_42 = data.iter().all(|&x| x == 42);
                print_bool(all_42);
                crate::logger::early_print("\n");
            }
            Err(_) => {
                crate::logger::early_print("FAIL (allocation error)\n");
            }
        }
    }

    crate::logger::early_print("TLS tests: COMPLETE\n");
}

/// Test 5: Async Runtime - Executor and tasks
pub fn test_async_runtime() {
    crate::logger::early_print("\n=== TEST 5: Async Runtime ===\n");

    use exo_std::async_rt::{Executor, block_on};

    // Test executor creation
    crate::logger::early_print("[5.1] Create executor... ");
    let mut executor = Executor::new();
    print_bool(executor.ready_count() == 0);
    crate::logger::early_print("\n");

    // Test simple future
    crate::logger::early_print("[5.2] Execute simple future... ");
    async fn test_future() -> u32 {
        42
    }
    let result = block_on(test_future());
    print_bool(result == 42);
    crate::logger::early_print("\n");

    // Test task spawning
    crate::logger::early_print("[5.3] Spawn tasks... ");
    for _ in 0..5 {
        executor.spawn(async {
            // Simple task
        });
    }
    print_bool(executor.ready_count() == 5);
    crate::logger::early_print("\n");

    crate::logger::early_print("[5.4] Run executor... ");
    executor.run();
    print_bool(!executor.has_tasks());
    crate::logger::early_print("\n");

    crate::logger::early_print("Async Runtime tests: COMPLETE\n");
}

/// Test 6: Benchmarking - Performance measurement
pub fn test_benchmarking() {
    crate::logger::early_print("\n=== TEST 6: Benchmarking ===\n");

    use exo_std::bench::Benchmark;

    crate::logger::early_print("[6.1] Run simple benchmark... ");
    let result = Benchmark::new("test")
        .iterations(100)
        .run(|| {
            let _x = 1 + 1;
        });

    print_bool(result.iterations == 100);
    crate::logger::early_print("\n");

    crate::logger::early_print("[6.2] Check avg time > 0... ");
    print_bool(result.avg_nanos() > 0);
    crate::logger::early_print(" (");
    print_u64(result.avg_nanos());
    crate::logger::early_print("ns)\n");

    crate::logger::early_print("[6.3] Ops per second... ");
    let ops = result.ops_per_sec();
    print_bool(ops > 0.0);
    crate::logger::early_print("\n");

    crate::logger::early_print("Benchmarking tests: COMPLETE\n");
}

/// Test 7: IntrusiveList - Zero-allocation list
pub fn test_intrusive_list() {
    crate::logger::early_print("\n=== TEST 7: IntrusiveList ===\n");

    use exo_std::collections::IntrusiveList;

    let mut list: IntrusiveList<i32> = IntrusiveList::new();

    crate::logger::early_print("[7.1] Create empty list... ");
    print_bool(list.is_empty());
    crate::logger::early_print("\n");

    crate::logger::early_print("[7.2] List size is 0... ");
    print_bool(list.len() == 0);
    crate::logger::early_print("\n");

    crate::logger::early_print("IntrusiveList tests: COMPLETE\n");
}

/// Run all integration tests
pub fn run_all_tests() {
    crate::logger::early_print("\n");
    crate::logger::early_print("╔═══════════════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║   EXO_STD v0.3.0 - TESTS INTEGRATION RÉELS               ║\n");
    crate::logger::early_print("║   Running in Kernel - Real Hardware Conditions           ║\n");
    crate::logger::early_print("╚═══════════════════════════════════════════════════════════╝\n");

    test_hashmap();
    test_btreemap();
    test_futex();
    test_tls();
    test_async_runtime();
    test_benchmarking();
    test_intrusive_list();

    crate::logger::early_print("\n");
    crate::logger::early_print("╔═══════════════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║   ALL TESTS COMPLETE                                     ║\n");
    crate::logger::early_print("╚═══════════════════════════════════════════════════════════╝\n");
    crate::logger::early_print("\n");
}

/// Run specific benchmark tests
pub fn run_benchmarks() {
    crate::logger::early_print("\n");
    crate::logger::early_print("╔═══════════════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║   EXO_STD BENCHMARKS - Real Performance Tests            ║\n");
    crate::logger::early_print("╚═══════════════════════════════════════════════════════════╝\n");

    use exo_std::bench::Benchmark;

    // HashMap benchmark
    crate::logger::early_print("\n[BENCH] HashMap Insert (100 items x 1000 iterations)...\n");
    let result = Benchmark::new("HashMap Insert")
        .iterations(1000)
        .run(|| {
            let mut map = exo_std::collections::HashMap::new();
            for i in 0..100 {
                map.insert(i, i * 2);
            }
        });
    crate::logger::early_print("  Avg: ");
    print_u64(result.avg_nanos());
    crate::logger::early_print("ns | Min: ");
    print_u64(result.min_duration.as_nanos() as u64);
    crate::logger::early_print("ns | Max: ");
    print_u64(result.max_duration.as_nanos() as u64);
    crate::logger::early_print("ns\n");

    // BTreeMap benchmark
    crate::logger::early_print("\n[BENCH] BTreeMap Insert (50 items x 1000 iterations)...\n");
    let result = Benchmark::new("BTreeMap Insert")
        .iterations(1000)
        .run(|| {
            let mut tree = exo_std::collections::BTreeMap::new();
            for i in 0..50 {
                tree.insert(i, i * 3);
            }
        });
    crate::logger::early_print("  Avg: ");
    print_u64(result.avg_nanos());
    crate::logger::early_print("ns | Min: ");
    print_u64(result.min_duration.as_nanos() as u64);
    crate::logger::early_print("ns | Max: ");
    print_u64(result.max_duration.as_nanos() as u64);
    crate::logger::early_print("ns\n");

    // Futex benchmark
    crate::logger::early_print("\n[BENCH] FutexMutex Lock/Unlock (10000 iterations)...\n");
    let mutex = exo_std::sync::FutexMutex::new();
    let result = Benchmark::new("Futex Lock/Unlock")
        .iterations(10000)
        .run(|| {
            mutex.lock();
            mutex.unlock();
        });
    crate::logger::early_print("  Avg: ");
    print_u64(result.avg_nanos());
    crate::logger::early_print("ns | Min: ");
    print_u64(result.min_duration.as_nanos() as u64);
    crate::logger::early_print("ns | Max: ");
    print_u64(result.max_duration.as_nanos() as u64);
    crate::logger::early_print("ns\n");

    crate::logger::early_print("\n");
    crate::logger::early_print("╔═══════════════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║   BENCHMARKS COMPLETE                                    ║\n");
    crate::logger::early_print("╚═══════════════════════════════════════════════════════════╝\n");
    crate::logger::early_print("\n");
}
