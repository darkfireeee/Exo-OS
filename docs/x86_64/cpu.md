# 🔧 CPU Features

## Détection CPUID

```rust
pub fn detect_features() -> CpuFeatures {
    let mut features = CpuFeatures::empty();
    
    // CPUID leaf 1, ECX
    let (_, _, ecx, edx) = cpuid(1);
    
    if ecx & (1 << 0) != 0 { features |= CpuFeatures::SSE3; }
    if ecx & (1 << 9) != 0 { features |= CpuFeatures::SSSE3; }
    if ecx & (1 << 19) != 0 { features |= CpuFeatures::SSE4_1; }
    if ecx & (1 << 20) != 0 { features |= CpuFeatures::SSE4_2; }
    if ecx & (1 << 28) != 0 { features |= CpuFeatures::AVX; }
    
    // Extended features (leaf 7)
    let (_, ebx, ecx, _) = cpuid_count(7, 0);
    
    if ebx & (1 << 5) != 0 { features |= CpuFeatures::AVX2; }
    if ebx & (1 << 16) != 0 { features |= CpuFeatures::AVX512F; }
    
    features
}
```

## Features Supportées

| Feature | Usage dans Exo-OS |
|---------|-------------------|
| SSE2 | Baseline, memcpy optimisé |
| SSE4.2 | CRC32, POPCNT |
| AVX/AVX2 | Crypto, compression |
| AVX-512 | Optionnel, si disponible |
| RDTSC | Timestamps haute précision |
| RDTSCP | Timestamps avec CPU ID |
| XSAVE | Sauvegarde FPU/SIMD |
| PCID | TLB non-global |

## MSRs (Model-Specific Registers)

### MSRs Utilisés

```rust
pub const IA32_EFER: u32 = 0xC0000080;       // Extended Feature Enable
pub const IA32_STAR: u32 = 0xC0000081;       // SYSCALL selectors
pub const IA32_LSTAR: u32 = 0xC0000082;      // SYSCALL entry point
pub const IA32_FMASK: u32 = 0xC0000084;      // SYSCALL flag mask
pub const IA32_FS_BASE: u32 = 0xC0000100;    // FS base address
pub const IA32_GS_BASE: u32 = 0xC0000101;    // GS base address
pub const IA32_KERNEL_GS_BASE: u32 = 0xC0000102; // Kernel GS base (swapgs)
```

### API MSR

```rust
// Lecture MSR
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high);
    ((high as u64) << 32) | (low as u64)
}

// Écriture MSR
pub unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    asm!("wrmsr", in("ecx") msr, in("eax") low, in("edx") high);
}
```

## Per-CPU Data

```rust
#[repr(C)]
pub struct PerCpuData {
    /// Self pointer (pour accès via GS)
    pub self_ptr: *const PerCpuData,
    
    /// CPU ID
    pub cpu_id: u32,
    
    /// Thread courant
    pub current_thread: *mut Thread,
    
    /// Stack kernel
    pub kernel_stack: u64,
    
    /// TSS
    pub tss: *mut Tss,
}
```

### Accès via GS

```rust
// Initialisation
wrmsr(IA32_GS_BASE, &PER_CPU_DATA as *const _ as u64);

// Accès
pub fn current_cpu() -> &'static PerCpuData {
    unsafe {
        let ptr: *const PerCpuData;
        asm!("mov {}, gs:[0]", out(reg) ptr);
        &*ptr
    }
}
```
