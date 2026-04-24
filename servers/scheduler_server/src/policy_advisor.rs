#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SchedulingClass {
    Cfs,
    Batch,
    Idle,
    Realtime,
    Deadline,
}

impl SchedulingClass {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Cfs),
            1 => Some(Self::Batch),
            2 => Some(Self::Idle),
            3 => Some(Self::Realtime),
            4 => Some(Self::Deadline),
            _ => None,
        }
    }

    pub const fn as_u32(self) -> u32 {
        match self {
            Self::Cfs => 0,
            Self::Batch => 1,
            Self::Idle => 2,
            Self::Realtime => 3,
            Self::Deadline => 4,
        }
    }
}

#[derive(Clone, Copy)]
pub struct SchedulingProfile {
    pub class: SchedulingClass,
    pub nice: i8,
    pub priority_weight: u32,
    pub quantum_ms: u16,
}

pub struct PolicyAdvisor;

impl PolicyAdvisor {
    pub const fn new() -> Self {
        Self
    }

    pub fn recommend(
        &self,
        raw_nice: i32,
        latency_hint_ms: u16,
        class: SchedulingClass,
    ) -> SchedulingProfile {
        let nice = clamp_nice(raw_nice);
        let priority_weight = compute_weight(nice, class);
        let quantum_ms = compute_quantum_ms(class, latency_hint_ms);
        SchedulingProfile {
            class,
            nice,
            priority_weight,
            quantum_ms,
        }
    }
}

fn clamp_nice(raw_nice: i32) -> i8 {
    raw_nice.clamp(-20, 19) as i8
}

fn compute_weight(nice: i8, class: SchedulingClass) -> u32 {
    let base = match class {
        SchedulingClass::Cfs => 1024u32,
        SchedulingClass::Batch => 768u32,
        SchedulingClass::Idle => 128u32,
        SchedulingClass::Realtime => 4096u32,
        SchedulingClass::Deadline => 3072u32,
    };
    let bias = if nice < 0 {
        (-nice as u32).saturating_mul(96)
    } else {
        (nice as u32).saturating_mul(48)
    };
    if nice < 0 {
        base.saturating_add(bias)
    } else {
        base.saturating_sub(bias).max(16)
    }
}

fn compute_quantum_ms(class: SchedulingClass, latency_hint_ms: u16) -> u16 {
    match class {
        SchedulingClass::Cfs => latency_hint_ms.clamp(2, 32),
        SchedulingClass::Batch => latency_hint_ms.clamp(8, 64),
        SchedulingClass::Idle => latency_hint_ms.clamp(16, 128),
        SchedulingClass::Realtime => latency_hint_ms.clamp(1, 8),
        SchedulingClass::Deadline => latency_hint_ms.clamp(1, 4),
    }
}
