#![no_std]

pub mod exocordon;
// FIX-ROUTER-02 (exoos_ipc_incoherences.md §2) : router et load_balancer
// n'étaient déclarés ni dans lib.rs ni dans main.rs — code mort jamais compilé,
// ce qui a masqué le syscall hardcodé 302 et le passage de src_pid en position
// flags. Les modules sont désormais compilés avec le crate.
pub mod load_balancer;
pub mod router;
pub mod security_gate;
