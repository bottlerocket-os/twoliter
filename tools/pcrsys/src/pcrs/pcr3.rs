//! PCR 3: Option ROM Configuration and Data
//!
//! - AWS/VMware: Separator only
//! - Metal: Not predicted (varies)

use crate::error::Result;
use crate::platform::Platform;
use crate::predict::{extend_pcr_separator, PcrContext, PcrIndex, PcrRecord, PCR_INIT_VAL};

/// Predict PCR 3 value.
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    match ctx.platform {
        Platform::Aws | Platform::Vmware => Ok(Some((
            PcrIndex::Pcr3,
            PcrRecord::new(extend_pcr_separator(&PCR_INIT_VAL)),
        ))),
        Platform::Metal => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::predict::test_support::{MockCtx, SEPARATOR_HASH};
    use test_case::test_case;

    #[test_case(Platform::Aws, Some(SEPARATOR_HASH))]
    #[test_case(Platform::Vmware, Some(SEPARATOR_HASH))]
    #[test_case(Platform::Metal, None)]
    fn test_predict(platform: Platform, expected: Option<&str>) {
        let m = MockCtx::new();
        let ctx = m.build(platform);
        let result = predict(&ctx).unwrap();
        match expected {
            Some(hash) => {
                let (idx, r) = result.unwrap();
                assert_eq!(idx, PcrIndex::Pcr3);
                assert_eq!(r.sha256[0], hash);
            }
            None => assert!(result.is_none()),
        }
    }
}
