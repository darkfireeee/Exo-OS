#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SymbolRef {
    pub name_offset: u32,
    pub value: u64,
    pub size: u64,
}
