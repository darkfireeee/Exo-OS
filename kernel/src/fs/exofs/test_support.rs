//! Helpers de tests ExoFS pour remplacer les unwrap()/expect().

/// Extension commune pour `Result` et `Option` dans les tests.
pub trait TestUnwrapExt<T> {
    fn test_unwrap(self) -> T;
    fn test_expect(self, msg: &str) -> T;
}

impl<T> TestUnwrapExt<T> for Option<T> {
    fn test_unwrap(self) -> T {
        match self {
            Some(value) => value,
            None => panic!("test_unwrap: valeur absente"),
        }
    }

    fn test_expect(self, msg: &str) -> T {
        match self {
            Some(value) => value,
            None => panic!("{msg}"),
        }
    }
}

impl<T, E: core::fmt::Debug> TestUnwrapExt<T> for Result<T, E> {
    fn test_unwrap(self) -> T {
        match self {
            Ok(value) => value,
            Err(err) => panic!("test_unwrap: {err:?}"),
        }
    }

    fn test_expect(self, msg: &str) -> T {
        match self {
            Ok(value) => value,
            Err(err) => panic!("{msg}: {err:?}"),
        }
    }
}
