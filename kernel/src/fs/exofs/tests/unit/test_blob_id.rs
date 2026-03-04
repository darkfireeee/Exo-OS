//! Tests unitaires — BlobId Blake3 (spec 2.1).
#[cfg(test)]
mod tests {
    use crate::fs::exofs::core::BlobId;

    #[test]
    fn test_blob_id_distinct_for_different_data() {
        let a = BlobId([0u8; 32]);
        let b = BlobId([1u8; 32]);
        assert!(!a.ct_eq(&b));
    }
}
