//! Tests unitaires — core/ types fondamentaux.
#[cfg(test)]
mod tests {
    use crate::fs::exofs::core::*;

    #[test]
    fn test_epoch_id_monotone() {
        let a = EpochId(1);
        let b = EpochId(2);
        assert!(a < b);
    }

    #[test]
    fn test_blob_id_ct_eq_reflexive() {
        let id = BlobId([0x42u8; 32]);
        assert!(id.ct_eq(&id));
    }

    #[test]
    fn test_object_id_not_eq() {
        let a = ObjectId([0u8; 32]);
        let b = ObjectId([1u8; 32]);
        assert!(!a.ct_eq(&b));
    }
}
