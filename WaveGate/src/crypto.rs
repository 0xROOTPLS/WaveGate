use rcgen::{CertificateParams, KeyPair, DnType, SanType, Ia5String};
use std::fs;
use std::path::PathBuf;
use thiserror::Error;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("Failed to generate key pair: {0}")]
    KeyGeneration(String),
    #[error("Failed to generate certificate: {0}")]
    CertGeneration(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid DNS name: {0}")]
    InvalidDnsName(String),
}

pub struct CertificateData {
    pub cert_pem: String,
    pub key_pem: String,
    pub cert_base64: String,
    pub key_base64: String,
}

/// Get the data directory for storing certificates
pub fn get_data_dir() -> Result<PathBuf, CryptoError> {
    println!("get_data_dir called");
    let proj_dirs = directories::ProjectDirs::from("com", "wavegate", "WaveGate")
        .ok_or_else(|| {
            println!("ProjectDirs::from failed");
            CryptoError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not determine data directory"
            ))
        })?;

    let data_dir = proj_dirs.data_dir().to_path_buf();
    println!("Data dir: {:?}", data_dir);
    fs::create_dir_all(&data_dir)?;
    println!("Directory created/exists");
    Ok(data_dir)
}

/// Generate a new self-signed certificate and private key
pub fn generate_certificate() -> Result<CertificateData, CryptoError> {
    // Generate a new key pair
    let key_pair = KeyPair::generate()
        .map_err(|e| CryptoError::KeyGeneration(e.to_string()))?;

    // Set up certificate parameters
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, "WaveGate Server");
    params.distinguished_name.push(DnType::OrganizationName, "WaveGate");

    // Add subject alternative names for localhost and common local addresses
    let localhost = Ia5String::try_from("localhost")
        .map_err(|e| CryptoError::InvalidDnsName(e.to_string()))?;

    params.subject_alt_names = vec![
        SanType::DnsName(localhost),
        SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))),
        SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0))),
    ];

    // Generate the certificate
    let cert = params.self_signed(&key_pair)
        .map_err(|e| CryptoError::CertGeneration(e.to_string()))?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    // Base64 encode for display in UI
    let cert_base64 = BASE64.encode(cert.der());
    let key_base64 = BASE64.encode(key_pair.serialize_der());

    Ok(CertificateData {
        cert_pem,
        key_pem,
        cert_base64,
        key_base64,
    })
}

/// Save certificate and key to disk
pub fn save_certificate(cert_data: &CertificateData) -> Result<PathBuf, CryptoError> {
    let data_dir = get_data_dir()?;

    let cert_path = data_dir.join("server.crt");
    let key_path = data_dir.join("server.key");

    fs::write(&cert_path, &cert_data.cert_pem)?;
    fs::write(&key_path, &cert_data.key_pem)?;

    Ok(data_dir)
}

/// Load existing certificate and key from disk
pub fn load_certificate() -> Result<CertificateData, CryptoError> {
    let data_dir = get_data_dir()?;

    let cert_path = data_dir.join("server.crt");
    let key_path = data_dir.join("server.key");

    let cert_pem = fs::read_to_string(&cert_path)?;
    let key_pem = fs::read_to_string(&key_path)?;

    // Parse to get DER for base64 encoding
    let cert_der = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .next()
        .ok_or_else(|| CryptoError::CertGeneration("No certificate found in PEM".to_string()))?
        .map_err(|e| CryptoError::CertGeneration(e.to_string()))?;

    let key_der = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .map_err(|e| CryptoError::KeyGeneration(e.to_string()))?
        .ok_or_else(|| CryptoError::KeyGeneration("No private key found in PEM".to_string()))?;

    let cert_base64 = BASE64.encode(cert_der.as_ref());
    let key_base64 = BASE64.encode(key_der.secret_der());

    Ok(CertificateData {
        cert_pem,
        key_pem,
        cert_base64,
        key_base64,
    })
}

/// Check if certificate exists
pub fn certificate_exists() -> bool {
    if let Ok(data_dir) = get_data_dir() {
        let cert_path = data_dir.join("server.crt");
        let key_path = data_dir.join("server.key");
        cert_path.exists() && key_path.exists()
    } else {
        false
    }
}

/// Result of certificate initialization
pub struct CertInitResult {
    pub data: CertificateData,
    pub was_loaded: bool,
    pub path: PathBuf,
}

/// Initialize certificate - load existing or generate new
pub fn init_certificate() -> Result<CertInitResult, CryptoError> {
    let data_dir = get_data_dir()?;

    if certificate_exists() {
        let data = load_certificate()?;
        Ok(CertInitResult {
            data,
            was_loaded: true,
            path: data_dir,
        })
    } else {
        let data = generate_certificate()?;
        save_certificate(&data)?;
        Ok(CertInitResult {
            data,
            was_loaded: false,
            path: data_dir,
        })
    }
}
