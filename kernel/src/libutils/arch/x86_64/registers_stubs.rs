//! Stubs pour registres CPU (tests Windows uniquement)

pub fn read_cr3() -> usize { 0 }
pub fn write_cr3(_value: usize) {}
pub fn read_cr2() -> usize { 0 }
pub fn read_cr0() -> usize { 0 }
pub fn write_cr0(_value: usize) {}
pub fn read_cr4() -> usize { 0 }
pub fn write_cr4(_value: usize) {}
pub fn invalidate_page(_addr: usize) {}
pub fn enable_interrupts() {}
pub fn disable_interrupts() {}
pub fn read_flags() -> usize { 0 }
pub fn are_interrupts_enabled() -> bool { false }
pub fn interrupts_enabled() -> bool { false }
pub fn halt() {}
pub fn nop() {}
pub fn mfence() {}
pub fn sfence() {}
pub fn lfence() {}
pub fn rdtsc() -> u64 { 0 }
pub fn rdmsr(_msr: u32) -> u64 { 0 }
pub fn wrmsr(_msr: u32, _value: u64) {}
pub fn rdfsbase() -> usize { 0 }
pub fn wrfsbase(_value: usize) {}
pub fn rdgsbase() -> usize { 0 }
pub fn wrgsbase(_value: usize) {}
pub fn pause() {}
pub fn wbinvd() {}
pub fn cpuid(_leaf: u32) -> (u32, u32, u32, u32) { (0, 0, 0, 0) }
pub fn xgetbv(_xcr: u32) -> u64 { 0 }
pub fn xsetbv(_xcr: u32, _value: u64) {}
pub fn get_apic_id() -> u32 { 0 }

// Port I/O stubs
pub unsafe fn read_port_u8(_port: u16) -> u8 { 0 }
pub unsafe fn write_port_u8(_port: u16, _value: u8) {}
pub unsafe fn read_port_u16(_port: u16) -> u16 { 0 }
pub unsafe fn write_port_u16(_port: u16, _value: u16) {}
pub unsafe fn read_port_u32(_port: u16) -> u32 { 0 }
pub unsafe fn write_port_u32(_port: u16, _value: u32) {}

