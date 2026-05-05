#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LibraryRef<'a> {
    pub soname: &'a str,
}
