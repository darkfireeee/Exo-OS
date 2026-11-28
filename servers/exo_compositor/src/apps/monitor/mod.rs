//! # Moniteur Système & ExoBench — Diagnostics multi-graphes et profilage complet.

mod bench_view;
mod overview;
mod processes;
mod shell;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MonitorTab {
    Overview = 0,
    LiveGraphs = 1,
    Processes = 2,
    Bench = 3,
}

pub struct MonitorApp {
    pub tab: MonitorTab,
    pub scroll_proc: usize,
    pub scroll_svc: usize,
    pub scroll_bench: i32,
    pub scroll_y: i32,
    pub dragging_scrollbar: bool,
    pub selected_proc_pid: Option<u32>,
    pub bench_selected_mod: usize,
}

impl MonitorApp {
    pub fn new(initial: MonitorTab) -> Self {
        Self {
            tab: initial,
            scroll_proc: 0,
            scroll_svc: 0,
            scroll_bench: 0,
            scroll_y: 0,
            dragging_scrollbar: false,
            selected_proc_pid: None,
            bench_selected_mod: 0,
        }
    }
}
