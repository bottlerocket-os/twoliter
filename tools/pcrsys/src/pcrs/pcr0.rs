//! PCR 0: Firmware Code
//!
//! PCR 0 measurements vary by platform:
//! - AWS: Fixed value based on Nitro firmware
//! - VMware: Not predicted (EV_S_CRTM_VERSION contains firmware build string that
//!   varies across vSphere versions, e.g. "VM71.00V.21805430.B64.2305221826")
//! - Metal: Not predicted (varies by hardware vendor firmware)

use crate::error::Result;
use crate::platform::Platform;
use crate::predict::{PcrContext, PcrDigests, PcrIndex, PcrRecord};
use hex_literal::hex;

/// AWS Nitro PCR 0 value (fixed for all Nitro instances).
const AWS_PCR0: PcrDigests = PcrDigests {
    sha256: hex!("737f767a12f54e70eecbc8684011323ae2fe2dd9f90785577969d7a2013e8c12"),
    sha384: hex!("6e901b16932f6e036747d7a57696e4a2cc864008ebc016826ca1d7bd42ab5ac8286ccf49cde6c0284cbc4b63d978a2ec"),
};

/// Predict PCR 0 value.
///
/// Returns `None` for VMware/Metal platforms since values vary.
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    match ctx.platform {
        Platform::Aws => Ok(Some((PcrIndex::Pcr0, PcrRecord::new(AWS_PCR0)))),
        Platform::Vmware | Platform::Metal => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::predict::test_support::MockCtx;
    use test_case::test_case;

    #[test_case(
        Platform::Aws,
        Some("737f767a12f54e70eecbc8684011323ae2fe2dd9f90785577969d7a2013e8c12")
    )]
    #[test_case(Platform::Vmware, None)]
    #[test_case(Platform::Metal, None)]
    fn test_predict(platform: Platform, expected: Option<&str>) {
        let m = MockCtx::new();
        let ctx = m.build(platform);
        let result = predict(&ctx).unwrap();
        match expected {
            Some(hash) => {
                let (idx, r) = result.unwrap();
                assert_eq!(idx, PcrIndex::Pcr0);
                assert_eq!(r.sha256[0], hash);
            }
            None => assert!(result.is_none()),
        }
    }
}
