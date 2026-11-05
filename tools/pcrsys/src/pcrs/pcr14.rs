//! PCR 14: MOK variables (MokList, MokListX, MokListTrusted)

use crate::efi::{generate_null_esl_sha256, generate_x509_esl};
use crate::error::Result;
use crate::image::BootChain;
use crate::predict::{extend_pcr_data, PcrContext, PcrIndex, PcrRecord, PCR_INIT_VAL};

/// Predict PCR 14 value.
///
/// PCR 14 = extend(MokList ESL) -> extend(MokListX ESL) -> extend(MokListTrusted)
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    let shim = match &ctx.image.boot {
        // Direct UKI boot has no shim MOK measurements.
        BootChain::Uki(_) => return Ok(Some((PcrIndex::Pcr14, PcrRecord::new(PCR_INIT_VAL)))),
        BootChain::Grub(grub) => &grub.shim,
    };

    let vendor_cert = shim.vendor_cert();

    // MokList: X509 ESL containing vendor certificate
    let mok_list_esl = generate_x509_esl(vendor_cert)?;

    // MokListX: null SHA256 ESL (empty revocation list)
    let mok_list_x_esl = generate_null_esl_sha256();

    let mut pcr = extend_pcr_data(&PCR_INIT_VAL, &mok_list_esl);
    pcr = extend_pcr_data(&pcr, &mok_list_x_esl);
    // MokListTrusted: 0x01 (MOK_VARIABLE_INVERSE flag)
    pcr = extend_pcr_data(&pcr, &[0x01]);

    Ok(Some((PcrIndex::Pcr14, PcrRecord::new(pcr))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;
    use crate::predict::test_support::MockCtx;

    #[test]
    fn test_predict() {
        let m = MockCtx::new();
        let result = predict(&m.build(Platform::Aws)).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr14);
        assert_eq!(
            result.1.sha256[0],
            "44e95bbfbad86e58e97cf5c810c24d817177435677a5ffb8d0d1945b706a886f"
        );
    }
}
