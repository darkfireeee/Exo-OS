#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignatureState {
    Unsigned,
    Present,
}

pub fn detect_signature_note(image: &[u8]) -> SignatureState {
    if image.windows(8).any(|w| w == b"EXOSIG\0\0") {
        SignatureState::Present
    } else {
        SignatureState::Unsigned
    }
}
