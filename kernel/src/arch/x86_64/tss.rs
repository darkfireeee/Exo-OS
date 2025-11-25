//! Task State Segment (TSS)
//! 
//! Manages interrupt stack tables and privilege transitions.

#[repr(C, packed)]
pub struct TaskStateSegment {
    _reserved1: u32,
    pub rsp0: u64,        // Stack pointer for ring 0
    pub rsp1: u64,        // Stack pointer for ring 1
    pub rsp2: u64,        // Stack pointer for ring 2
    _reserved2: u64,
    pub ist: [u64; 7],    // Interrupt Stack Table
    _reserved3: u64,
    _reserved4: u16,
    pub iomap_base: u16,
}

impl TaskStateSegment {
    pub const fn new() -> Self {
        TaskStateSegment {
            _reserved1: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            _reserved2: 0,
            ist: [0; 7],
            _reserved3: 0,
            _reserved4: 0,
            iomap_base: 0,
        }
    }
}

static mut TSS: TaskStateSegment = TaskStateSegment::new();

pub fn init() {
    log::info!("TSS initialized");
}
