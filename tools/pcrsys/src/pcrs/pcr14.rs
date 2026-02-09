//! PCR 14: MOK variables (MokList, MokListX, MokListTrusted)

use crate::efi::{generate_null_esl_sha256, generate_x509_esl};
use crate::error::Result;
use crate::pe::extract_vendor_cert;
use crate::predict::{extend_pcr, PcrContext, PcrIndex, PcrRecord, PCR_INIT_VAL};
use sha2::{Digest, Sha256};

/// Predict PCR 14 value.
///
/// PCR 14 = extend(MokList ESL) -> extend(MokListX ESL) -> extend(MokListTrusted)
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    let vendor_cert = extract_vendor_cert(ctx.shim)?;

    // MokList: X509 ESL containing vendor certificate
    let mok_list_esl = generate_x509_esl(&vendor_cert)?;
    let mok_list_digest: [u8; 32] = Sha256::digest(&mok_list_esl).into();

    // MokListX: null SHA256 ESL (empty revocation list)
    let mok_list_x_esl = generate_null_esl_sha256();
    let mok_list_x_digest: [u8; 32] = Sha256::digest(&mok_list_x_esl).into();

    // MokListTrusted: 0x01 (MOK_VARIABLE_INVERSE flag)
    let mok_list_trusted_digest: [u8; 32] = Sha256::digest([0x01u8]).into();

    let mut pcr = extend_pcr(&PCR_INIT_VAL, &mok_list_digest);
    pcr = extend_pcr(&pcr, &mok_list_x_digest);
    pcr = extend_pcr(&pcr, &mok_list_trusted_digest);

    Ok(Some((PcrIndex::Pcr14, PcrRecord::new(pcr))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;
    use crate::predict::test_support::{build_test_shim, MockCtx};

    #[test]
    fn test_predict() {
        let shim = build_test_shim();
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(Platform::Aws)
            .efi_vars(&m.efi_vars)
            .partitions(&m.layout)
            .shim(&shim)
            .build();
        let result = predict(&ctx).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr14);
        assert_eq!(
            result.1.sha256[0],
            "44e95bbfbad86e58e97cf5c810c24d817177435677a5ffb8d0d1945b706a886f"
        );
    }

    #[test]
    fn test_predict_invalid_pe() {
        let invalid_pe = vec![0u8; 100];
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(Platform::Aws)
            .efi_vars(&m.efi_vars)
            .partitions(&m.layout)
            .shim(&invalid_pe)
            .build();
        let err = predict(&ctx).unwrap_err();
        assert!(err.to_string().contains("PE32+") || err.to_string().contains("parse"));
    }
}
