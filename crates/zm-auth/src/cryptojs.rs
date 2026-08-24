use aes::Aes256;
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use md5::{Digest, Md5};
use rand::RngCore;
use zm_core::{Result, ZmError};

const PASSPHRASE: &[u8] = b"lzYW5qaXVqa";
type Aes256CbcEnc = cbc::Encryptor<Aes256>;

pub fn encrypt_password(password: &str) -> Result<String> {
    let mut salt = [0_u8; 8];
    rand::rng().fill_bytes(&mut salt);
    encrypt_password_with_salt(password, salt)
}

pub fn encrypt_password_with_salt(password: &str, salt: [u8; 8]) -> Result<String> {
    let derived = evp_bytes_to_key(PASSPHRASE, &salt, 48);
    let ciphertext = Aes256CbcEnc::new_from_slices(&derived[..32], &derived[32..48])
        .map_err(|e| ZmError::Protocol(format!("初始化密码加密器失败：{e}")))?
        .encrypt_padded_vec_mut::<Pkcs7>(password.as_bytes());
    let mut output = b"Salted__".to_vec();
    output.extend_from_slice(&salt);
    output.extend_from_slice(&ciphertext);
    Ok(STANDARD.encode(output))
}

fn evp_bytes_to_key(password: &[u8], salt: &[u8], output_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(output_len);
    let mut previous = Vec::new();
    while out.len() < output_len {
        let mut hasher = Md5::new();
        if !previous.is_empty() {
            hasher.update(&previous);
        }
        hasher.update(password);
        hasher.update(salt);
        previous = hasher.finalize().to_vec();
        out.extend_from_slice(&previous);
    }
    out.truncate(output_len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_cryptojs_openssl_format() {
        let value = encrypt_password_with_salt("test123", [0, 1, 2, 3, 4, 5, 6, 7]).unwrap();
        assert_eq!(value, "U2FsdGVkX18AAQIDBAUGB1biWbCBgRtcOwoi58UcS7I=");
        assert_eq!(
            STANDARD.decode(value).unwrap()[..16],
            *b"Salted__\0\x01\x02\x03\x04\x05\x06\x07"
        );
    }
}
