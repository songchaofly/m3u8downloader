use crate::{AppError, Result};
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use base64::Engine;

pub fn decode_key(value: &str) -> Result<[u8; 16]> {
    let s = value.trim().trim_start_matches("0x");
    let bytes = if s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        hex::decode(s).map_err(|_| AppError::new("KEY_INVALID", "无效十六进制密钥"))?
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(|_| AppError::new("KEY_INVALID", "密钥必须为十六进制或 Base64"))?
    };
    bytes
        .try_into()
        .map_err(|_| AppError::new("KEY_INVALID", "AES-128 密钥必须为 16 字节"))
}
pub fn iv(value: Option<&str>, sequence: u64) -> Result<[u8; 16]> {
    match value {
        None => Ok((sequence as u128).to_be_bytes()),
        Some(s) => {
            let s = s.trim().trim_start_matches("0x").trim_start_matches("0X");
            if s.len() > 32 || s.is_empty() {
                return Err(AppError::new("KEY_INVALID", "IV 不能超过 128 位"));
            }
            let padded = format!("{:0>32}", s);
            let b = hex::decode(padded)
                .map_err(|_| AppError::new("KEY_INVALID", "IV 必须为十六进制"))?;
            Ok(b.try_into().unwrap())
        }
    }
}
pub fn decrypt(ciphertext: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Result<Vec<u8>> {
    cbc::Decryptor::<aes::Aes128>::new(key.into(), iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .map_err(|_| AppError::new("DECRYPT_FAILED", "密钥、IV 或媒体内容无效"))
}
pub fn validate_media(bytes: &[u8], subtitle: bool) -> Result<()> {
    if subtitle {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| AppError::new("SEGMENT_INVALID", "字幕不是 UTF-8"))?;
        if !text.trim_start_matches('\u{feff}').starts_with("WEBVTT") && !text.contains("-->") {
            return Err(AppError::new("SEGMENT_INVALID", "无效 WebVTT 字幕"));
        }
    } else {
        if bytes.len() < 188 || bytes[0] != 0x47 || (bytes.len() >= 376 && bytes[188] != 0x47) {
            return Err(AppError::new(
                "SEGMENT_INVALID",
                "媒体不是受支持的 MPEG-TS，或解密结果无效",
            ));
        }
    }
    Ok(())
}
