//! fadvise compatibility hints.

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advice {
    Normal = 0,
    Random = 1,
    Sequential = 2,
    WillNeed = 3,
    DontNeed = 4,
    NoReuse = 5,
}

pub fn is_prefetch_hint(advice: Advice) -> bool {
    matches!(advice, Advice::WillNeed | Advice::Sequential)
}

pub fn is_eviction_hint(advice: Advice) -> bool {
    matches!(advice, Advice::DontNeed)
}
