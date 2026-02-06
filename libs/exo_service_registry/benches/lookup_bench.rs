//! Benchmarks pour lookup de services
//!
//! Teste les performances de:
//! - Lookup avec cache hit
//! - Lookup avec cache miss
//! - Lookup avec bloom filter rejection
//! - Registration de services
//! - Heartbeat updates

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use exo_service_registry::{Registry, RegistryConfig, ServiceName, ServiceInfo};

/// Benchmark: lookup avec cache hit (hot path)
fn bench_lookup_cache_hit(c: &mut Criterion) {
    let mut group = c.benchmark_group("lookup_cache_hit");

    for cache_size in [10, 100, 1000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(cache_size),
            &cache_size,
            |b, &size| {
                let config = RegistryConfig::default().with_cache_size(size);
                let mut registry = Registry::with_config(config);

                // Prépare le service
                let name = ServiceName::new("test_service").unwrap();
                let info = ServiceInfo::new("/tmp/test.sock");
                registry.register(name.clone(), info).unwrap();

                // Premier lookup pour peupler le cache
                let _ = registry.lookup(&name);

                b.iter(|| {
                    // Lookup depuis le cache (devrait être <100ns)
                    let result = registry.lookup(black_box(&name));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: lookup avec cache miss
fn bench_lookup_cache_miss(c: &mut Criterion) {
    let mut group = c.benchmark_group("lookup_cache_miss");

    for num_services in [10, 100, 1000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(num_services),
            &num_services,
            |b, &n| {
                let mut registry = Registry::new();

                // Enregistre N services
                for i in 0..n {
                    let name = ServiceName::new(&format!("service_{}", i)).unwrap();
                    let info = ServiceInfo::new(&format!("/tmp/service_{}.sock", i));
                    registry.register(name, info).unwrap();
                }

                // Service à lookup
                let target = ServiceName::new("service_0").unwrap();

                // Clear cache pour forcer cache miss
                let config = RegistryConfig::default();
                let mut fresh_registry = Registry::with_config(config);
                for i in 0..n {
                    let name = ServiceName::new(&format!("service_{}", i)).unwrap();
                    let info = ServiceInfo::new(&format!("/tmp/service_{}.sock", i));
                    fresh_registry.register(name, info).unwrap();
                }

                b.iter(|| {
                    let result = fresh_registry.lookup(black_box(&target));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: lookup avec bloom filter rejection
fn bench_lookup_bloom_rejection(c: &mut Criterion) {
    let mut group = c.benchmark_group("lookup_bloom_rejection");

    for bloom_size in [1000, 10000, 100000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(bloom_size),
            &bloom_size,
            |b, &size| {
                let config = RegistryConfig::default().with_bloom_size(size);
                let mut registry = Registry::with_config(config);

                // Enregistre quelques services
                for i in 0..10 {
                    let name = ServiceName::new(&format!("service_{}", i)).unwrap();
                    let info = ServiceInfo::new(&format!("/tmp/service_{}.sock", i));
                    registry.register(name, info).unwrap();
                }

                // Service inexistant (devrait être rejeté par bloom filter)
                let nonexistent = ServiceName::new("nonexistent_service").unwrap();

                b.iter(|| {
                    let result = registry.lookup(black_box(&nonexistent));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: registration de services
fn bench_registration(c: &mut Criterion) {
    let mut group = c.benchmark_group("registration");

    for backend_type in ["inmemory"] {
        group.bench_with_input(
            BenchmarkId::from_parameter(backend_type),
            &backend_type,
            |b, _| {
                b.iter_with_setup(
                    || {
                        // Setup: nouveau registry à chaque itération
                        Registry::new()
                    },
                    |mut registry| {
                        // Benchmark: enregistre un service
                        let name = ServiceName::new("test_service").unwrap();
                        let info = ServiceInfo::new("/tmp/test.sock");
                        let result = registry.register(black_box(name), black_box(info));
                        black_box(result);
                    },
                );
            },
        );
    }

    group.finish();
}

/// Benchmark: heartbeat update
fn bench_heartbeat(c: &mut Criterion) {
    let mut group = c.benchmark_group("heartbeat");

    for num_services in [10, 100, 1000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(num_services),
            &num_services,
            |b, &n| {
                let mut registry = Registry::new();

                // Enregistre N services
                for i in 0..n {
                    let name = ServiceName::new(&format!("service_{}", i)).unwrap();
                    let info = ServiceInfo::new(&format!("/tmp/service_{}.sock", i));
                    registry.register(name, info).unwrap();
                }

                let target = ServiceName::new("service_0").unwrap();

                b.iter(|| {
                    let result = registry.heartbeat(black_box(&target));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: list all services
fn bench_list_services(c: &mut Criterion) {
    let mut group = c.benchmark_group("list_services");

    for num_services in [10, 100, 1000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(num_services),
            &num_services,
            |b, &n| {
                let mut registry = Registry::new();

                // Enregistre N services
                for i in 0..n {
                    let name = ServiceName::new(&format!("service_{}", i)).unwrap();
                    let info = ServiceInfo::new(&format!("/tmp/service_{}.sock", i));
                    registry.register(name, info).unwrap();
                }

                b.iter(|| {
                    let result = registry.list();
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: mixed workload (lookup + register + heartbeat)
fn bench_mixed_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_workload");

    group.bench_function("realistic", |b| {
        b.iter_with_setup(
            || {
                let mut registry = Registry::new();

                // Setup: enregistre 100 services
                for i in 0..100 {
                    let name = ServiceName::new(&format!("service_{}", i)).unwrap();
                    let info = ServiceInfo::new(&format!("/tmp/service_{}.sock", i));
                    registry.register(name, info).unwrap();
                }

                registry
            },
            |mut registry| {
                // Workload réaliste:
                // - 80% lookups (dont 50% cache hits)
                // - 10% registrations
                // - 10% heartbeats

                for i in 0..100 {
                    match i % 10 {
                        0 => {
                            // Registration (10%)
                            let name = ServiceName::new(&format!("new_service_{}", i)).unwrap();
                            let info = ServiceInfo::new(&format!("/tmp/new_{}.sock", i));
                            let _ = registry.register(name, info);
                        }
                        1 => {
                            // Heartbeat (10%)
                            let name = ServiceName::new(&format!("service_{}", i % 100)).unwrap();
                            let _ = registry.heartbeat(&name);
                        }
                        _ => {
                            // Lookup (80%)
                            let name = ServiceName::new(&format!("service_{}", i % 100)).unwrap();
                            let _ = registry.lookup(&name);
                        }
                    }
                }
            },
        );
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_lookup_cache_hit,
    bench_lookup_cache_miss,
    bench_lookup_bloom_rejection,
    bench_registration,
    bench_heartbeat,
    bench_list_services,
    bench_mixed_workload,
);

criterion_main!(benches);
