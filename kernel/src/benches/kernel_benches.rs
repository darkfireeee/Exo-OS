//! Benchmarks pour les performances du noyau Exo-OS
//!
//! Ce module contient des benchmarks pour mesurer les performances
//! des différents composants du noyau.

use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Benchmark de l'affichage VGA
pub fn vga_display_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("vga_display");
    
    group.bench_function("clear_screen", |b| {
        b.iter(|| {
            // Simuler l'effacement de l'écran VGA
            black_box({
                // Code simulé - dans un vrai benchmark on appellerait la vraie fonction
                for _ in 0..2000 {
                    black_box(());
                }
            });
        })
    });
    
    group.bench_function("write_banner", |b| {
        b.iter(|| {
            // Simuler l'écriture d'un banner
            black_box({
                // Code simulé - dans un vrai benchmark on appellerait la vraie fonction
                for _ in 0..1000 {
                    black_box(());
                }
            });
        })
    });
    
    group.finish();
}

/// Benchmark de la gestion des interruptions
pub fn interrupt_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("interrupt");
    
    group.bench_function("interrupt_handler", |b| {
        b.iter(|| {
            // Simuler le traitement d'une interruption
            black_box({
                let mut counter = 0;
                for i in 0..100 {
                    counter += i;
                    black_box(counter);
                }
            });
        })
    });
    
    group.bench_function("interrupt_disable_enable", |b| {
        b.iter(|| {
            // Simuler la désactivation/activation des interruptions
            black_box({
                // Simuler disable_interrupts()
                for _ in 0..10 {
                    black_box(());
                }
                // Simuler enable_interrupts()
                for _ in 0..10 {
                    black_box(());
                }
            });
        })
    });
    
    group.finish();
}

/// Benchmark du scheduler
pub fn scheduler_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("scheduler");
    
    group.bench_function("context_switch", |b| {
        b.iter(|| {
            // Simuler un changement de contexte
            black_box({
                let mut regs = [0u64; 16];
                for i in 0..regs.len() {
                    regs[i] = i as u64;
                    black_box(regs[i]);
                }
                // Simuler la sauvegarde/restauration des registres
                for i in 0..regs.len() {
                    black_box(regs[i]);
                }
            });
        })
    });
    
    group.bench_function("schedule", |b| {
        b.iter(|| {
            // Simuler l'ordonnancement d'une tâche
            black_box({
                let mut tasks = vec![1, 2, 3, 4, 5];
                tasks.sort_by_key(|&x| x);
                black_box(tasks);
            });
        })
    });
    
    group.finish();
}

/// Benchmark de la mémoire
pub fn memory_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory");
    
    group.bench_function("frame_allocate", |b| {
        b.iter(|| {
            // Simuler l'allocation d'un cadre mémoire
            black_box({
                let mut addr = 0x1000;
                for _ in 0..100 {
                    addr += 0x1000;
                    black_box(addr);
                }
            });
        })
    });
    
    group.bench_function("page_table_walk", |b| {
        b.iter(|| {
            // Simuler une promenade dans la table de pages
            black_box({
                let virt_addr = 0xDEADBEEF;
                for shift in [39, 30, 21, 12] {
                    let index = (virt_addr >> shift) & 0x1FF;
                    black_box(index);
                }
            });
        })
    });
    
    group.bench_function("heap_alloc", |b| {
        b.iter(|| {
            // Simuler une allocation sur le tas
            black_box({
                let mut buffer = Vec::<u8>::with_capacity(1024);
                for i in 0..1024 {
                    buffer.push((i % 256) as u8);
                }
                black_box(buffer.len());
            });
        })
    });
    
    group.finish();
}

/// Benchmark des appels système
pub fn syscall_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("syscall");
    
    group.bench_function("syscall_dispatch", |b| {
        b.iter(|| {
            // Simuler la distribution d'un appel système
            black_box({
                let syscall_num = 1; // write
                match syscall_num {
                    0 => black_box(0), // read
                    1 => black_box(1), // write
                    2 => black_box(2), // open
                    3 => black_box(3), // close
                    60 => black_box(60), // exit
                    _ => black_box(-1),
                }
            });
        })
    });
    
    group.bench_function("serial_write", |b| {
        b.iter(|| {
            // Simuler l'écriture série
            black_box({
                let message = b"Hello from Exo-OS kernel!";
                for &byte in message {
                    // Simuler l'écriture d'un octet
                    black_box(byte);
                }
            });
        })
    });
    
    group.finish();
}

/// Benchmark global du démarrage du noyau
pub fn kernel_boot_benchmark(c: &mut Criterion) {
    c.bench_function("kernel_boot_sequence", |b| {
        b.iter(|| {
            // Simuler la séquence de démarrage complète
            black_box({
                // 1. Initialisation série
                for _ in 0..10 { black_box(()); }
                
                // 2. Initialisation mémoire
                for _ in 0..50 { black_box(()); }
                
                // 3. Initialisation GDT/IDT
                for _ in 0..30 { black_box(()); }
                
                // 4. Initialisation scheduler
                for _ in 0..20 { black_box(()); }
                
                // 5. Initialisation IPC
                for _ in 0..15 { black_box(()); }
                
                // 6. Initialisation syscall
                for _ in 0..10 { black_box(()); }
                
                // 7. Initialisation drivers
                for _ in 0..25 { black_box(()); }
                
                // 8. Affichage VGA
                for _ in 0..100 { black_box(()); }
            });
        })
    });
}

// Définir les groupes de benchmarks
criterion_group!(
    benches,
    vga_display_benchmark,
    interrupt_benchmark,
    scheduler_benchmark,
    memory_benchmark,
    syscall_benchmark,
    kernel_boot_benchmark
);

criterion_main!(benches);