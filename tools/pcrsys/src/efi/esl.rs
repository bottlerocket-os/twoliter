//! ESL (EFI Signature List) types and Secure Boot operations.

use crate::error::Result;
use hex_literal::hex;
use snafu::{ensure_whatever, FromString, Whatever};
use uuid::Uuid;

/// EFI Signature List header size: 16 (type GUID) + 4 (list size) + 4 (header size) + 4 (sig size).
const ESL_HEADER_SIZE: usize = 28;

/// Size of the signature owner GUID field.
const SIGNATURE_OWNER_SIZE: usize = 16;

/// Size of a SHA-256 hash.
const SHA256_SIZE: usize = 32;

/// GUID for shim lock variables (MokList, SbatLevel, etc.).
pub const SHIM_LOCK_GUID: Uuid = Uuid::from_bytes(hex!("605dab50 e046 4300 abb6 3dd810dd8b23"));

/// GUID for global EFI variables (SecureBoot, PK, KEK).
pub const EFI_GLOBAL_VARIABLE_GUID: Uuid =
    Uuid::from_bytes(hex!("8be4df61 93ca 11d2 aa0d 00e098032b8c"));

/// GUID for image security database variables (db, dbx).
pub const EFI_IMAGE_SECURITY_DATABASE_GUID: Uuid =
    Uuid::from_bytes(hex!("d719b2cb 3d3a 4596 a3bc dad00e67656f"));

/// GUID for X.509 certificate type in signature lists.
pub const EFI_CERT_X509_GUID: Uuid = Uuid::from_bytes(hex!("a5c059a1 94e4 4aa7 87b5 ab155c2bf072"));

/// GUID for SHA-256 hash type in signature lists.
pub const EFI_CERT_SHA256_GUID: Uuid =
    Uuid::from_bytes(hex!("c1c41626 504c 4092 aca9 41f936934328"));

/// Parsed EFI Signature List header.
#[derive(Debug)]
pub struct EslHeader {
    sig_type: [u8; 16],
    /// Size of each signature entry.
    pub sig_size: usize,
    /// Offset where signatures start.
    pub sig_start: usize,
}

impl EslHeader {
    /// Parse and validate an ESL header.
    pub fn parse(esl: &[u8]) -> Result<Self> {
        ensure_whatever!(
            esl.len() >= ESL_HEADER_SIZE,
            "ESL too small: {} bytes",
            esl.len()
        );

        let sig_type: [u8; 16] = esl[0..16].try_into().unwrap();
        let header_size = u32::from_le_bytes(esl[20..24].try_into().unwrap()) as usize;
        let sig_size = u32::from_le_bytes(esl[24..28].try_into().unwrap()) as usize;

        ensure_whatever!(
            header_size == 0,
            "unsupported non-zero ESL header size: {header_size}"
        );
        ensure_whatever!(sig_size > 0, "ESL signature size is zero");

        ensure_whatever!(
            esl.len() >= ESL_HEADER_SIZE + sig_size,
            "ESL signature extends beyond data"
        );

        Ok(Self {
            sig_type,
            sig_size,
            sig_start: ESL_HEADER_SIZE,
        })
    }

    /// Validate that the signature type is X.509 certificate.
    pub fn ensure_x509(&self) -> Result<()> {
        ensure_whatever!(
            self.sig_type == EFI_CERT_X509_GUID.to_bytes_le(),
            "ESL signature type is not X.509"
        );
        Ok(())
    }
}

/// Convert an ASCII string to UTF-16LE encoding.
pub fn string_to_utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect()
}

/// Generate EFI variable data structure for PCR measurement.
///
/// Format: `GUID + name_len(u64) + data_len(u64) + UTF-16LE_name + data`
pub fn generate_efi_variable_data(guid: &Uuid, name: &str, data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    result.extend_from_slice(&guid.to_bytes_le());
    result.extend_from_slice(&(name.len() as u64).to_le_bytes());
    result.extend_from_slice(&(data.len() as u64).to_le_bytes());
    result.extend_from_slice(&string_to_utf16le(name));
    result.extend_from_slice(data);
    result
}

/// Generate an X.509 EFI Signature List for a DER certificate.
///
/// Used for MokList measurements in PCR 14.
pub fn generate_x509_esl(cert: &[u8]) -> Result<Vec<u8>> {
    let list_size: u32 = (ESL_HEADER_SIZE + SIGNATURE_OWNER_SIZE + cert.len())
        .try_into()
        .map_err(|_| Whatever::without_source("certificate too large for ESL".into()))?;
    let sig_size: u32 = (SIGNATURE_OWNER_SIZE + cert.len())
        .try_into()
        .map_err(|_| Whatever::without_source("certificate too large for ESL".into()))?;

    let mut result = Vec::new();
    // EFI_CERT_TYPE_X509_GUID
    result.extend_from_slice(&EFI_CERT_X509_GUID.to_bytes_le());
    // SignatureListSize
    result.extend_from_slice(&list_size.to_le_bytes());
    // SignatureHeaderSize: 0
    result.extend_from_slice(&0u32.to_le_bytes());
    // SignatureSize
    result.extend_from_slice(&sig_size.to_le_bytes());
    // SHIM_LOCK_GUID as owner
    result.extend_from_slice(&SHIM_LOCK_GUID.to_bytes_le());
    // Certificate data
    result.extend_from_slice(cert);
    Ok(result)
}

/// Generate a null SHA-256 EFI Signature List.
///
/// Used for MokListX measurements in PCR 14 when the variable is unset.
pub fn generate_null_esl_sha256() -> Vec<u8> {
    let list_size = (ESL_HEADER_SIZE + SIGNATURE_OWNER_SIZE + SHA256_SIZE) as u32;
    let sig_size = (SIGNATURE_OWNER_SIZE + SHA256_SIZE) as u32;

    let mut result = Vec::new();
    // EFI_CERT_SHA256_GUID
    result.extend_from_slice(&EFI_CERT_SHA256_GUID.to_bytes_le());
    // SignatureListSize
    result.extend_from_slice(&list_size.to_le_bytes());
    // SignatureHeaderSize: 0
    result.extend_from_slice(&0u32.to_le_bytes());
    // SignatureSize
    result.extend_from_slice(&sig_size.to_le_bytes());
    // SHIM_LOCK_GUID as owner
    result.extend_from_slice(&SHIM_LOCK_GUID.to_bytes_le());
    // SignatureData: 32 bytes of zeros
    result.extend_from_slice(&[0u8; SHA256_SIZE]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_esl_header_parse() {
        let mut esl = vec![0u8; 28 + 16 + 4];
        esl[0..16].copy_from_slice(&EFI_CERT_X509_GUID.to_bytes_le());
        esl[24..28].copy_from_slice(&20u32.to_le_bytes()); // sig_size = 16 + 4

        let header = EslHeader::parse(&esl).unwrap();
        assert_eq!(header.sig_size, 20);
        assert_eq!(header.sig_start, 28);
        header.ensure_x509().unwrap();
    }

    #[test]
    fn test_esl_header_too_small() {
        let esl = vec![0u8; 20];
        assert!(EslHeader::parse(&esl).is_err());
    }

    #[test]
    fn test_esl_header_extends_beyond() {
        let mut esl = vec![0u8; 28];
        esl[0..16].copy_from_slice(&EFI_CERT_X509_GUID.to_bytes_le());
        esl[24..28].copy_from_slice(&100u32.to_le_bytes());
        let err = EslHeader::parse(&esl).unwrap_err();
        assert!(err.to_string().contains("extends beyond"));
    }

    #[test]
    fn test_esl_header_wrong_type() {
        let mut esl = vec![0u8; 28 + 16 + 4];
        esl[0..16].copy_from_slice(&[0xFF; 16]); // Wrong GUID
        esl[24..28].copy_from_slice(&20u32.to_le_bytes());
        let header = EslHeader::parse(&esl).unwrap();
        let err = header.ensure_x509().unwrap_err();
        assert!(err.to_string().contains("not X.509"));
    }

    #[test]
    fn test_esl_header_zero_size() {
        let mut esl = vec![0u8; 28];
        esl[0..16].copy_from_slice(&EFI_CERT_X509_GUID.to_bytes_le());
        esl[24..28].copy_from_slice(&0u32.to_le_bytes());
        let err = EslHeader::parse(&esl).unwrap_err();
        assert!(err.to_string().contains("zero"));
    }

    #[test]
    fn test_esl_header_nonzero_header_size() {
        let mut esl = vec![0u8; 28 + 16 + 4 + 4];
        esl[0..16].copy_from_slice(&EFI_CERT_X509_GUID.to_bytes_le());
        esl[20..24].copy_from_slice(&4u32.to_le_bytes()); // header_size = 4
        esl[24..28].copy_from_slice(&20u32.to_le_bytes()); // sig_size = 20
        let err = EslHeader::parse(&esl).unwrap_err();
        assert!(err.to_string().contains("non-zero ESL header size"));
    }

    #[test]
    fn test_string_to_utf16le() {
        let result = string_to_utf16le("PK");
        assert_eq!(result, vec![0x50, 0x00, 0x4b, 0x00]); // 'P' 'K' in UTF-16LE
    }

    #[test]
    fn test_string_to_utf16le_empty() {
        let result = string_to_utf16le("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_generate_efi_variable_data() {
        let data = generate_efi_variable_data(&EFI_GLOBAL_VARIABLE_GUID, "PK", &[0x01]);
        // 16 (GUID) + 8 (name_len) + 8 (data_len) + 4 (UTF16 "PK") + 1 (data)
        assert_eq!(data.len(), 16 + 8 + 8 + 4 + 1);
        // Check name_len = 2
        assert_eq!(&data[16..24], &2u64.to_le_bytes());
        // Check data_len = 1
        assert_eq!(&data[24..32], &1u64.to_le_bytes());
    }

    #[test]
    fn test_generate_x509_esl() {
        let cert = vec![0x30, 0x82, 0x01, 0x00]; // Mock DER certificate header
        let esl = generate_x509_esl(&cert).unwrap();
        assert_eq!(
            esl.len(),
            ESL_HEADER_SIZE + SIGNATURE_OWNER_SIZE + cert.len()
        );
        // Check SignatureListSize
        let list_size = u32::from_le_bytes(esl[16..20].try_into().unwrap());
        assert_eq!(
            list_size as usize,
            ESL_HEADER_SIZE + SIGNATURE_OWNER_SIZE + cert.len()
        );
        // Verify ESL hash for regression detection
        use sha2::{Digest, Sha256};
        assert_eq!(
            hex::encode(Sha256::digest(&esl)),
            "5139edaf56516863817921bba251c5faa1b6974cd39922c474e53a50d0ffceac"
        );
    }

    #[test]
    fn test_generate_null_esl_sha256() {
        let esl = generate_null_esl_sha256();
        let expected_len = ESL_HEADER_SIZE + SIGNATURE_OWNER_SIZE + SHA256_SIZE;
        assert_eq!(esl.len(), expected_len);
        // Check SignatureListSize
        let list_size = u32::from_le_bytes(esl[16..20].try_into().unwrap());
        assert_eq!(list_size as usize, expected_len);
    }
}
