//! The Qt build's crypto (this crate's FFI, called by RustCryptoService.cpp) and
//! the egui build's (`crypto-kit`) must agree byte for byte, or a mailbox request
//! sealed by one cannot be opened by the other. Checked in both directions.
use snapclone_crypto::*;

#[test]
fn session_keys_and_aead_agree_with_crypto_kit() {
    let (a_sec, a_pub) = crypto_kit::x25519_keypair();
    let (b_sec, b_pub) = crypto_kit::x25519_keypair();
    // Session key: Qt side derives from (a, B); egui side from (b, A).
    let mut qt_key = [0u8; 32];
    let err = unsafe { sc_derive_session_key(a_sec.as_ptr(), b_pub.as_ptr(), qt_key.as_mut_ptr()) };
    assert_eq!(err as i32, CryptoError::Ok as i32);
    let shared = crypto_kit::x25519_shared(&b_sec, &a_pub);
    let kit_key = crypto_kit::derive_session_key(shared.as_ref(), b"snapclone-v1-session");
    assert_eq!(qt_key, *kit_key, "the two builds derive different session keys");

    // egui seals, Qt opens.
    let plain = b"CONNREQ\nsc-aabbccdd\n203.0.113.7:9876\n203.0.113.7:41000\n1790000000\n";
    let sealed = crypto_kit::seal(&kit_key, plain);
    let mut out = vec![0u8; sealed.len()];
    let mut n = 0usize;
    let err = unsafe {
        sc_decrypt_aes256gcm(qt_key.as_ptr(), sealed.as_ptr(), sealed.len(), out.as_mut_ptr(), out.len(), &mut n)
    };
    assert_eq!(err as i32, CryptoError::Ok as i32, "the Qt side could not open an egui-sealed request");
    assert_eq!(&out[..n], plain);

    // Qt seals, egui opens.
    let mut ct = vec![0u8; 12 + plain.len() + 16];
    let mut m = 0usize;
    let err = unsafe {
        sc_encrypt_aes256gcm(qt_key.as_ptr(), plain.as_ptr(), plain.len(), ct.as_mut_ptr(), ct.len(), &mut m)
    };
    assert_eq!(err as i32, CryptoError::Ok as i32);
    assert_eq!(crypto_kit::open(&kit_key, &ct[..m]).as_deref(), Some(&plain[..]), "egui could not open a Qt-sealed request");
}
