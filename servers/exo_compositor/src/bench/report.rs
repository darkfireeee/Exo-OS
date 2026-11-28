//! ExoBench suite orchestrator, final report formatter, and ExoFS report exporter.

use crate::assets::FontSet;
use crate::bench::{kernel_ipc, memory_cpu, storage_ui, BenchCategory, BenchMetric};

#[derive(Clone, Debug)]
pub struct ModuleBenchReport {
    pub category: BenchCategory,
    pub score_100: u32,
    pub elapsed_us: u64,
    pub metrics: Vec<BenchMetric>,
}

pub struct BenchSuite {
    pub reports: Vec<ModuleBenchReport>,
    pub global_score: u32,
    pub total_elapsed_us: u64,
    pub run_count: u32,
    pub last_export_path: String,
}

impl BenchSuite {
    pub fn new() -> Self {
        Self {
            reports: Vec::new(),
            global_score: 0,
            total_elapsed_us: 0,
            run_count: 0,
            last_export_path: String::new(),
        }
    }

    pub fn run_all(&mut self, fonts: &FontSet) {
        self.reports.clear();
        for cat in BenchCategory::ALL {
            self.reports.push(run_category(cat, fonts));
        }
        self.recompute_summary();
        self.run_count += 1;
    }

    pub fn run_module(&mut self, cat: BenchCategory, fonts: &FontSet) {
        let rep = run_category(cat, fonts);
        if let Some(existing) = self.reports.iter_mut().find(|r| r.category == cat) {
            *existing = rep;
        } else {
            self.reports.push(rep);
        }
        self.recompute_summary();
        self.run_count += 1;
    }

    fn recompute_summary(&mut self) {
        if self.reports.is_empty() {
            self.global_score = 0;
            self.total_elapsed_us = 0;
            return;
        }
        let sum: u32 = self.reports.iter().map(|r| r.score_100).sum();
        self.global_score = sum / (self.reports.len() as u32);
        self.total_elapsed_us = self.reports.iter().map(|r| r.elapsed_us).sum();
    }

    pub fn grade(&self) -> &'static str {
        match self.global_score {
            90..=100 => "A+",
            80..=89 => "A",
            70..=79 => "B+",
            60..=69 => "B",
            45..=59 => "C",
            _ => "D",
        }
    }

    pub fn format_final_report(&self, hostname: &str) -> String {
        let mut out = String::new();
        out.push_str("================================================================\n");
        out.push_str("              RAPPORT DE PERFORMANCE EXOOS (EXOBENCH)           \n");
        out.push_str("================================================================\n");
        out.push_str(&format!("Machine       : {}\n", hostname));
        out.push_str(&format!("Score global  : {}/100 (Note {})\n", self.global_score, self.grade()));
        out.push_str(&format!(
            "Durée totale  : {}.{:02} ms ({} modules mesurés)\n\n",
            self.total_elapsed_us / 1000,
            (self.total_elapsed_us % 1000) / 10,
            self.reports.len()
        ));
        for rep in &self.reports {
            out.push_str(&format!(
                "--- [{}]  Score: {}/100  ({} µs) ---\n",
                rep.category.title_fr(),
                rep.score_100,
                rep.elapsed_us
            ));
            for m in &rep.metrics {
                out.push_str(&format!(
                    "  * {:<28} : {:<16} [score {:>3}/100]  {}\n",
                    m.name, m.value_text, m.score_100, m.detail
                ));
            }
            out.push('\n');
        }
        out.push_str("================================================================\n");
        out
    }

    pub fn export_report(&mut self, hostname: &str) -> Result<String, &'static str> {
        let path = String::from("/tmp/exo_perf_report.txt");
        let text = self.format_final_report(hostname);
        std::fs::write(&path, text.as_bytes()).map_err(|_| "Écriture refusée sur /tmp")?;
        self.last_export_path = path.clone();
        Ok(path)
    }
}

fn run_category(cat: BenchCategory, fonts: &FontSet) -> ModuleBenchReport {
    match cat {
        BenchCategory::KernelSyscalls => kernel_ipc::bench_kernel_syscalls(),
        BenchCategory::IpcSecurity => kernel_ipc::bench_ipc_security(),
        BenchCategory::MemoryCpu => memory_cpu::bench_memory_cpu(),
        BenchCategory::ExoFsStorage => storage_ui::bench_exofs_storage(),
        BenchCategory::GraphicsUi => storage_ui::bench_graphics_ui(fonts),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_benchmark_suite_measures_all_five_modules() {
        let fonts = FontSet::load();
        let mut suite = BenchSuite::new();
        suite.run_all(&fonts);
        assert_eq!(suite.reports.len(), 5);
        assert!(suite.global_score > 0);
        let rep = suite.format_final_report("exo-test");
        assert!(rep.contains("Stockage ExoFS"));
        assert!(rep.contains("IPC & ExoCordon"));
    }
}
