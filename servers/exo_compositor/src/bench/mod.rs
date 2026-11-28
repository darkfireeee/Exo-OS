//! # `ExoBench` — Outil de mesure de performances multi-modules d'ExoOS.

mod kernel_ipc;
mod memory_cpu;
mod report;
mod storage_ui;

pub use report::{BenchSuite, ModuleBenchReport};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchCategory {
    KernelSyscalls,
    IpcSecurity,
    MemoryCpu,
    ExoFsStorage,
    GraphicsUi,
}

impl BenchCategory {
    pub const ALL: [Self; 5] = [
        Self::KernelSyscalls,
        Self::IpcSecurity,
        Self::MemoryCpu,
        Self::ExoFsStorage,
        Self::GraphicsUi,
    ];

    pub const fn title_fr(self) -> &'static str {
        match self {
            Self::KernelSyscalls => "Noyau & Appels Système",
            Self::IpcSecurity => "IPC & ExoCordon",
            Self::MemoryCpu => "Mémoire & CPU",
            Self::ExoFsStorage => "Stockage ExoFS",
            Self::GraphicsUi => "Graphique & Compositeur 2D",
        }
    }

    pub const fn title_en(self) -> &'static str {
        match self {
            Self::KernelSyscalls => "Kernel & Syscalls",
            Self::IpcSecurity => "IPC & ExoCordon",
            Self::MemoryCpu => "Memory & CPU",
            Self::ExoFsStorage => "ExoFS Storage",
            Self::GraphicsUi => "Graphics & 2D Compositor",
        }
    }
}

#[derive(Clone, Debug)]
pub struct BenchMetric {
    pub name: String,
    pub value_text: String,
    pub score_100: u32,
    pub detail: String,
}

pub(crate) fn now_ns() -> u64 {
    #[cfg(target_os = "exo")]
    {
        #[repr(C)]
        #[derive(Default)]
        struct Ts { tv_sec: i64, tv_nsec: i64 }
        let mut ts = Ts::default();
        let _ = unsafe {
            exo_syscall_abi::syscall2(exo_syscall_abi::SYS_CLOCK_GETTIME, 1, &mut ts as *mut Ts as u64)
        };
        (ts.tv_sec.max(0) as u64).saturating_mul(1_000_000_000).saturating_add(ts.tv_nsec.max(0) as u64)
    }
    #[cfg(not(target_os = "exo"))]
    {
        use std::sync::OnceLock;
        use std::time::Instant;
        static EPOCH: OnceLock<Instant> = OnceLock::new();
        EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }
}
