//! PCR 7: Secure Boot Policy

use crate::efi::{
    generate_efi_variable_data, EslHeader, EFI_GLOBAL_VARIABLE_GUID,
    EFI_IMAGE_SECURITY_DATABASE_GUID, SHIM_LOCK_GUID,
};
use crate::error::Result;
use crate::pe::{extract_sbat_level, extract_vendor_cert};
use crate::platform::Platform;
use crate::predict::{
    extend_pcr, extend_pcr_separator, PcrContext, PcrIndex, PcrRecord, PCR_INIT_VAL,
};
use sha2::{Digest, Sha256};
use snafu::{OptionExt, ResultExt};

/// VMware SignatureOwner GUID used when enrolling Secure Boot keys via OVF.
const VMWARE_SIGNATURE_OWNER_GUID: [u8; 16] = [
    0x5b, 0xe9, 0xd5, 0xa3, 0x8f, 0x0a, 0x53, 0x47, 0x87, 0x35, 0x44, 0x5a, 0xfb, 0x70, 0x8f, 0x62,
];

/// Predict PCR 7 value.
///
/// PCR 7 measures Secure Boot policy:
/// 1. SecureBoot variable (0x01)
/// 2. PK, KEK, db, dbx variables
/// 3. Separator
/// 4. db authority (certificate used to verify shim)
/// 5. SbatLevel (from shim's .sbatlevel section)
/// 6. MokListRT (vendor certificate from shim)
///
/// Platform differences:
/// - AWS/Metal: uses SignatureOwner GUID from efi-vars.json
/// - VMware: uses VMware's SignatureOwner GUID for enrolled keys
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    let vendor_cert = extract_vendor_cert(ctx.shim)?;
    let sbat_level = extract_sbat_level(ctx.shim)?;

    // Get EFI variables
    let pk = ctx
        .efi_vars
        .get("PK")
        .whatever_context("PK variable not found")?;
    let kek = ctx
        .efi_vars
        .get("KEK")
        .whatever_context("KEK variable not found")?;
    let db = ctx
        .efi_vars
        .get("db")
        .whatever_context("db variable not found")?;
    let dbx = ctx
        .efi_vars
        .get("dbx")
        .whatever_context("dbx variable not found")?;

    let pk_data = hex::decode(&pk.data).whatever_context("invalid PK hex")?;
    let kek_data = hex::decode(&kek.data).whatever_context("invalid KEK hex")?;
    let db_data = hex::decode(&db.data).whatever_context("invalid db hex")?;
    let dbx_data = hex::decode(&dbx.data).whatever_context("invalid dbx hex")?;

    let pcr = match ctx.platform {
        Platform::Aws | Platform::Metal => predict_variant(
            &pk_data,
            &kek_data,
            &db_data,
            &dbx_data,
            &vendor_cert,
            &sbat_level,
        )?,
        Platform::Vmware => {
            // Replace SignatureOwner GUIDs with VMware's GUID
            let pk_data = replace_signature_owner_guid(&pk_data, &VMWARE_SIGNATURE_OWNER_GUID)?;
            let kek_data = replace_signature_owner_guid(&kek_data, &VMWARE_SIGNATURE_OWNER_GUID)?;
            let db_data = replace_signature_owner_guid(&db_data, &VMWARE_SIGNATURE_OWNER_GUID)?;
            let dbx_data = replace_signature_owner_guid(&dbx_data, &VMWARE_SIGNATURE_OWNER_GUID)?;
            predict_variant(
                &pk_data,
                &kek_data,
                &db_data,
                &dbx_data,
                &vendor_cert,
                &sbat_level,
            )?
        }
    };

    Ok(Some((PcrIndex::Pcr7, PcrRecord::new(pcr))))
}

/// Predict PCR 7 for a specific set of Secure Boot variables.
fn predict_variant(
    pk_data: &[u8],
    kek_data: &[u8],
    db_data: &[u8],
    dbx_data: &[u8],
    vendor_cert: &[u8],
    sbat_level: &[u8],
) -> Result<[u8; 32]> {
    // 1. SecureBoot = 0x01
    let secure_boot_data =
        generate_efi_variable_data(&EFI_GLOBAL_VARIABLE_GUID, "SecureBoot", &[0x01]);
    let mut pcr = extend_pcr(&PCR_INIT_VAL, &Sha256::digest(&secure_boot_data).into());

    // 2. PK
    let pk_var_data = generate_efi_variable_data(&EFI_GLOBAL_VARIABLE_GUID, "PK", pk_data);
    pcr = extend_pcr(&pcr, &Sha256::digest(&pk_var_data).into());

    // 3. KEK
    let kek_var_data = generate_efi_variable_data(&EFI_GLOBAL_VARIABLE_GUID, "KEK", kek_data);
    pcr = extend_pcr(&pcr, &Sha256::digest(&kek_var_data).into());

    // 4. db
    let db_var_data = generate_efi_variable_data(&EFI_IMAGE_SECURITY_DATABASE_GUID, "db", db_data);
    pcr = extend_pcr(&pcr, &Sha256::digest(&db_var_data).into());

    // 5. dbx
    let dbx_var_data =
        generate_efi_variable_data(&EFI_IMAGE_SECURITY_DATABASE_GUID, "dbx", dbx_data);
    pcr = extend_pcr(&pcr, &Sha256::digest(&dbx_var_data).into());

    // 6. Separator
    pcr = extend_pcr_separator(&pcr);

    // 7. db authority - certificate used to verify shim
    // Assumes single cert in db per Bottlerocket sbkeys profile.
    // If multiple signature lists with different sizes exist, only the first is extracted.
    let db_sig = extract_first_sig_from_esl(db_data)?;
    let db_authority = generate_efi_variable_data(&EFI_IMAGE_SECURITY_DATABASE_GUID, "db", &db_sig);
    pcr = extend_pcr(&pcr, &Sha256::digest(&db_authority).into());

    // 8. SbatLevel
    let sbat_var_data = generate_efi_variable_data(&SHIM_LOCK_GUID, "SbatLevel", sbat_level);
    pcr = extend_pcr(&pcr, &Sha256::digest(&sbat_var_data).into());

    // 9. MokListRT - owner GUID + vendor certificate
    let mut mok_data = SHIM_LOCK_GUID.to_bytes_le().to_vec();
    mok_data.extend_from_slice(vendor_cert);
    let mok_var_data = generate_efi_variable_data(&SHIM_LOCK_GUID, "MokListRT", &mok_data);
    pcr = extend_pcr(&pcr, &Sha256::digest(&mok_var_data).into());

    Ok(pcr)
}

/// Replace the SignatureOwner GUID in an EFI Signature List.
///
/// ESL format has signatures starting at offset 28 + header_size,
/// where each signature begins with a 16-byte owner GUID.
fn replace_signature_owner_guid(esl: &[u8], new_owner: &[u8; 16]) -> Result<Vec<u8>> {
    let header = EslHeader::parse(esl)?;
    let mut result = esl.to_vec();

    // Replace owner GUID in each signature
    let mut offset = header.sig_start;
    while offset + 16 <= result.len() && offset + header.sig_size <= result.len() {
        result[offset..offset + 16].copy_from_slice(new_owner);
        offset += header.sig_size;
    }

    Ok(result)
}

/// Extract the first signature (owner GUID + certificate) from an EFI Signature List.
fn extract_first_sig_from_esl(esl: &[u8]) -> Result<Vec<u8>> {
    let header = EslHeader::parse(esl)?;
    header.ensure_x509()?;
    Ok(esl[header.sig_start..header.sig_start + header.sig_size].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::efi::{EfiVar, EfiVars, EFI_CERT_X509_GUID};
    use crate::predict::test_support::{build_test_shim, MockCtx};

    #[test]
    fn test_extract_first_sig_from_esl() {
        // Build a minimal ESL with owner GUID + 4-byte certificate
        let mut esl = vec![0u8; 28 + 16 + 4];
        esl[0..16].copy_from_slice(&EFI_CERT_X509_GUID.to_bytes_le());
        esl[24..28].copy_from_slice(&20u32.to_le_bytes());
        esl[28..44].copy_from_slice(&[0x01; 16]);
        esl[44..48].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let sig = extract_first_sig_from_esl(&esl).unwrap();
        // Should include owner GUID + cert
        assert_eq!(sig.len(), 20);
        assert_eq!(&sig[0..16], &[0x01; 16]);
        assert_eq!(&sig[16..20], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    fn build_x509_esl() -> Vec<u8> {
        let mut esl = vec![0u8; 28 + 16 + 4];
        esl[0..16].copy_from_slice(&EFI_CERT_X509_GUID.to_bytes_le());
        esl[24..28].copy_from_slice(&20u32.to_le_bytes());
        esl[28 + 16..].copy_from_slice(&[0x30, 0x82, 0x00, 0x01]);
        esl
    }

    fn build_efi_vars() -> EfiVars {
        let global = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
        let secdb = "d719b2cb-3d3a-4596-a3bc-dad00e67656f";
        EfiVars {
            variables: vec![
                EfiVar::new("PK", global, hex::encode(build_x509_esl())),
                EfiVar::new("KEK", global, hex::encode(build_x509_esl())),
                EfiVar::new("db", secdb, hex::encode(build_x509_esl())),
                EfiVar::new("dbx", secdb, hex::encode(build_x509_esl())),
            ],
        }
    }

    #[test]
    fn test_predict_aws() {
        let shim = build_test_shim();
        let efi_vars = build_efi_vars();
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(Platform::Aws)
            .efi_vars(&efi_vars)
            .partitions(&m.layout)
            .shim(&shim)
            .build();
        let result = predict(&ctx).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr7);
        assert_eq!(
            result.1.sha256[0],
            "5a99dde72337a05276bb35eb66e37598708cbd3474e83adb36d100fc911da942"
        );
    }

    #[test]
    fn test_predict_vmware() {
        let shim = build_test_shim();
        let efi_vars = build_efi_vars();
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(Platform::Vmware)
            .efi_vars(&efi_vars)
            .partitions(&m.layout)
            .shim(&shim)
            .build();
        let result = predict(&ctx).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr7);
        assert_eq!(
            result.1.sha256[0],
            "7deb2373bc1e620b541d0d780987dc8080bdcbb03ae5db06b1357abd5dc18c06"
        );
    }

    #[test]
    fn test_predict_missing_pk() {
        let shim = build_test_shim();
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(Platform::Aws)
            .efi_vars(&m.efi_vars)
            .partitions(&m.layout)
            .shim(&shim)
            .build();
        let err = predict(&ctx).unwrap_err();
        assert!(err.to_string().contains("PK"));
    }

    #[test]
    fn test_predict_invalid_hex() {
        let shim = build_test_shim();
        let global = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
        let secdb = "d719b2cb-3d3a-4596-a3bc-dad00e67656f";
        let efi_vars = EfiVars {
            variables: vec![
                EfiVar::new("PK", global, "ZZZZ"),
                EfiVar::new("KEK", global, "01"),
                EfiVar::new("db", secdb, hex::encode(build_x509_esl())),
                EfiVar::new("dbx", secdb, "04"),
            ],
        };
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(Platform::Aws)
            .efi_vars(&efi_vars)
            .partitions(&m.layout)
            .shim(&shim)
            .build();
        let err = predict(&ctx).unwrap_err();
        assert!(err.to_string().contains("invalid") || err.to_string().contains("hex"));
    }
}
