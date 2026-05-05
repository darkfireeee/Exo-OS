#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DynamicInfo {
    pub needed_count: u16,
    pub strtab: u64,
    pub symtab: u64,
}
