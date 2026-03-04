//! Tests unitaires — EpochRecord taille et layout (spec ONDISK-06).
#[cfg(test)]
mod tests {
    use crate::fs::exofs::epoch::epoch_record::EpochRecord;
    use core::mem::size_of;

    #[test]
    fn test_epoch_record_size_104() {
        assert_eq!(size_of::<EpochRecord>(), 104,
            "EpochRecord doit faire exactement 104 octets (spec 2.4 ONDISK-06)");
    }
}
