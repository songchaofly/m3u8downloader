use m3u8_core::crypto::{decode_key, decrypt, iv, validate_media};
#[test]
fn iv_uses_media_sequence_in_big_endian() {
    let actual = iv(None, 100).unwrap();
    assert_eq!(actual[15], 100);
    assert_eq!(&actual[..15], &[0; 15]);
    assert_eq!(iv(Some("0x64"), 0).unwrap(), actual);
}
#[test]
fn invalid_keys_and_padding_fail_before_success() {
    assert!(decode_key("bad").is_err());
    assert_eq!(
        decode_key("000102030405060708090a0b0c0d0e0f").unwrap(),
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
    );
    assert!(decrypt(&[0; 16], &[0; 16], &[0; 16]).is_err());
    assert!(validate_media(b"<html>error</html>", false).is_err());
}
