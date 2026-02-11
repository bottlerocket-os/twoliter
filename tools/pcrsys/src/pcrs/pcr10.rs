//! PCR 10: Zero (unused)

use crate::error::Result;
use crate::predict::{PcrContext, PcrIndex, PcrRecord, PCR_INIT_VAL};

/// Predict PCR 10 value (always zero, unused).
pub fn predict(_ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    Ok(Some((PcrIndex::Pcr10, PcrRecord::new(PCR_INIT_VAL))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;
    use crate::predict::test_support::MockCtx;

    #[test]
    fn test_predict() {
        let m = MockCtx::new();
        let ctx = m.build(Platform::Aws);
        let result = predict(&ctx).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr10);
        assert_eq!(result.1.sha256.len(), 1);
        assert_eq!(
            result.1.sha256[0],
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
    }
}
