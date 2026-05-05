#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolveError {
    NotFound,
    Ambiguous,
}
