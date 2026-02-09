//! PCR 0: Firmware Code
//!
//! PCR 0 measurements vary by platform:
//! - AWS: Fixed value based on Nitro firmware
//! - VMware: Not predicted (EV_S_CRTM_VERSION contains firmware build string that
//!   varies across vSphere versions, e.g. "VM71.00V.21805430.B64.2305221826")
//! - Metal: Not predicted (varies by hardware vendor firmware)

use crate::error::Result;
use crate::platform::Platform;
use crate::predict::{PcrContext, PcrIndex, PcrRecord};

/// AWS Nitro PCR 0 value (fixed for all Nitro instances).
const AWS_PCR0: [u8; 32] = [
    0x73, 0x7f, 0x76, 0x7a, 0x12, 0xf5, 0x4e, 0x70, 0xee, 0xcb, 0xc8, 0x68, 0x40, 0x11, 0x32, 0x3a,
    0xe2, 0xfe, 0x2d, 0xd9, 0xf9, 0x07, 0x85, 0x57, 0x79, 0x69, 0xd7, 0xa2, 0x01, 0x3e, 0x8c, 0x12,
];

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
