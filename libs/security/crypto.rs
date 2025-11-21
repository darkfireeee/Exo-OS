// libs/exo_std/src/security/crypto.rs
use exo_crypto::{kyber_keypair, kyber_encaps, kyber_decaps, dilithium_keypair, dilithium_sign, dilithium_verify};
use crate::io::{Result as IoResult, IoError};

/// Système cryptographique pour Exo-OS
pub struct CryptoSystem;

impl CryptoSystem {
    /// Génère une paire de clés Kyber
    pub fn generate_kyber_keys() -> IoResult<(Vec<u8>, Vec<u8>)> {
let mut public_key = [0u8; exo_crypto::KYBER_PUBLICKEYBYTES];
let mut secret_key = [0u8; exo_crypto::KYBER_SECRETKEYBYTES];

if kyber_keypair(&mut public_key, &mut secret_key) {
Ok((
public_key.to_vec(),
secret_key.to_vec(),
))
} else {
Err(IoError::Other)
}
}

/// Effectue un encapsulation Kyber
pub fn kyber_encapsulate(
public_key: &[u8],
shared_secret: &mut [u8],
) -> IoResult<Vec<u8>> {
if public_key.len() != exo_crypto::KYBER_PUBLICKEYBYTES {
return Err(IoError::InvalidInput);
}

let mut pk = [0u8; exo_crypto::KYBER_PUBLICKEYBYTES];
let mut ct = [0u8; exo_crypto::KYBER_CIPHERTEXTBYTES];
let mut ss = [0u8; exo_crypto::KYBER_BYTES];

pk.copy_from_slice(public_key);

if kyber_encaps(&mut ct, &mut ss, &pk) {
shared_secret.copy_from_slice(&ss);
Ok(ct.to_vec())
} else {
Err(IoError::CryptoError)
}
}

/// Effectue une décapsulation Kyber
pub fn kyber_decapsulate(
ciphertext: &[u8],
secret_key: &[u8],
shared_secret: &mut [u8],
) -> IoResult<()> {
if ciphertext.len() != exo_crypto::KYBER_CIPHERTEXTBYTES ||
secret_key.len() != exo_crypto::KYBER_SECRETKEYBYTES {
return Err(IoError::InvalidInput);
}

let mut ct = [0u8; exo_crypto::KYBER_CIPHERTEXTBYTES];
let mut sk = [0u8; exo_crypto::KYBER_SECRETKEYBYTES];
let mut ss = [0u8; exo_crypto::KYBER_BYTES];

ct.copy_from_slice(ciphertext);
sk.copy_from_slice(secret_key);

if kyber_decaps(&mut ss, &ct, &sk) {
shared_secret.copy_from_slice(&ss);
Ok(())
} else {
Err(IoError::CryptoError)
}
}

/// Génère une paire de clés Dilithium
pub fn generate_dilithium_keys() -> IoResult<(Vec<u8>, Vec<u8>)> {
let mut public_key = [0u8; exo_crypto::DILITHIUM_PUBLICKEYBYTES];
let mut secret_key = [0u8; exo_crypto::DILITHIUM_SECRETKEYBYTES];

if dilithium_keypair(&mut public_key, &mut secret_key) {
Ok((
public_key.to_vec(),
secret_key.to_vec(),
))
} else {
Err(IoError::Other)
}
}

/// Signe un message avec Dilithium
pub fn sign_message(
message: &[u8],
secret_key: &[u8],
) -> IoResult<Vec<u8>> {
if secret_key.len() != exo_crypto::DILITHIUM_SECRETKEYBYTES {
return Err(IoError::InvalidInput);
}

let mut sk = [0u8; exo_crypto::DILITHIUM_SECRETKEYBYTES];
let mut sig = [0u8; exo_crypto::DILITHIUM_BYTES];

sk.copy_from_slice(secret_key);

if dilithium_sign(&mut sig, message, &sk) {
Ok(sig.to_vec())
} else {
Err(IoError::CryptoError)
}
}

/// Vérifie une signature Dilithium
pub fn verify_signature(
message: &[u8],
signature: &[u8],
public_key: &[u8],
) -> bool {
if signature.len() != exo_crypto::DILITHIUM_BYTES ||
public_key.len() != exo_crypto::DILITHIUM_PUBLICKEYBYTES {
return false;
}

let mut sig = [0u8; exo_crypto::DILITHIUM_BYTES];
let mut pk = [0u8; exo_crypto::DILITHIUM_PUBLICKEYBYTES];

sig.copy_from_slice(signature);
pk.copy_from_slice(public_key);

dilithium_verify(&sig, message, &pk)
}

/// Chiffre avec XChaCha20-Poly1305
pub fn encrypt_data(
plaintext: &[u8],
key: &[u8; 32],
nonce: &[u8; 24],
aad: &[u8],
) -> IoResult<Vec<u8>> {
let mut ciphertext = vec![0u8; plaintext.len() + exo_crypto::POLY1305_TAGBYTES];

if exo_crypto::XChaCha20::encrypt_aead(
plaintext,
aad,
nonce,
key,
&mut ciphertext,
) {
Ok(ciphertext)
} else {
Err(IoError::CryptoError)
}
}

/// Déchiffre avec XChaCha20-Poly1305
pub fn decrypt_data(
ciphertext: &[u8],
key: &[u8; 32],
nonce: &[u8; 24],
aad: &[u8],
) -> IoResult<Vec<u8>> {
if ciphertext.len() < exo_crypto::POLY1305_TAGBYTES {
return Err(IoError::InvalidInput);
}

let mut plaintext = vec![0u8; ciphertext.len() - exo_crypto::POLY1305_TAGBYTES];

if exo_crypto::XChaCha20::decrypt_aead(
ciphertext,
aad,
nonce,
key,
&mut plaintext,
) {
Ok(plaintext)
} else {
Err(IoError::CryptoError)
}
}

/// Génère des clés aléatoires post-quantiques
pub fn generate_pq_keypair() -> IoResult<(Vec<u8>, Vec<u8>)> {
// Combiner Kyber et Dilithium pour une sécurité post-quantique complète
let (kyber_pk, kyber_sk) = self.generate_kyber_keys()?;
let (dilithium_pk, dilithium_sk) = self.generate_dilithium_keys()?;

// Combiner les clés publiques
let mut public_key = Vec::with_capacity(kyber_pk.len() + dilithium_pk.len());
public_key.extend_from_slice(&kyber_pk);
public_key.extend_from_slice(&dilithium_pk);

// Combiner les clés secrètes
let mut secret_key = Vec::with_capacity(kyber_sk.len() + dilithium_sk.len());
secret_key.extend_from_slice(&kyber_sk);
secret_key.extend_from_slice(&dilithium_sk);

Ok((public_key, secret_key))
}
}

#[cfg(test)]
mod tests {
use super::*;

#[test]
fn test_kyber_roundtrip() {
let crypto = CryptoSystem;
let (public_key, secret_key) = crypto.generate_kyber_keys().unwrap();

// Générer un secret partagé
let mut shared_secret1 = [0u8; exo_crypto::KYBER_BYTES];
let ciphertext = crypto.kyber_encapsulate(&public_key, &mut shared_secret1).unwrap();

// Décapsuler avec la clé secrète
let mut shared_secret2 = [0u8; exo_crypto::KYBER_BYTES];
crypto.kyber_decapsulate(&ciphertext, &secret_key, &mut shared_secret2).unwrap();

// Vérifier que les secrets correspondent
assert_eq!(shared_secret1, shared_secret2);
}

#[test]
fn test_dilithium_signing() {
let crypto = CryptoSystem;
let (public_key, secret_key) = crypto.generate_dilithium_keys().unwrap();
let message = b"Hello, Exo-OS! This is a test message for Dilithium signatures.";

// Signer le message
let signature = crypto.sign_message(message, &secret_key).unwrap();

// Vérifier la signature
assert!(crypto.verify_signature(message, &signature, &public_key));

// Vérifier que la signature échoue avec un message modifié
let modified_message = b"Hello, Exo-OS! This is a modified test message.";
assert!(!crypto.verify_signature(modified_message, &signature, &public_key));
}

#[test]
fn test_chacha20_aead() {
let crypto = CryptoSystem;
let key = [42u8; 32];
let nonce = [17u8; 24];
let plaintext = b"Hello, Exo-OS! This is a secret message.";
let aad = b"associated authenticated data";

// Chiffrer
let ciphertext = crypto.encrypt_data(plaintext, &key, &nonce, aad).unwrap();

// Déchiffrer
let decrypted = crypto.decrypt_data(&ciphertext, &key, &nonce, aad).unwrap();

// Vérifier que les données correspondent
assert_eq!(decrypted, plaintext);

// Vérifier que le déchiffrement échoue avec des AAD incorrects
let wrong_aad = b"wrong associated data";
assert!(crypto.decrypt_data(&ciphertext, &key, &nonce, wrong_aad).is_err());
}

#[test]
fn test_pq_keypair() {
let crypto = CryptoSystem;
let (public_key, secret_key) = crypto.generate_pq_keypair().unwrap();

// Vérifier les tailles minimales
assert!(public_key.len() >= exo_crypto::KYBER_PUBLICKEYBYTES + exo_crypto::DILITHIUM_PUBLICKEYBYTES);
assert!(secret_key.len() >= exo_crypto::KYBER_SECRETKEYBYTES + exo_crypto::DILITHIUM_SECRETKEYBYTES);
}
}