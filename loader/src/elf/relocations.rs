#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationError {
    Unsupported,
    OutOfRange,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RelaEntry {
    pub offset: u64,
    pub info: u64,
    pub addend: i64,
}
