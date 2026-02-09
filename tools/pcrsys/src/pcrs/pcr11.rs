//! PCR 11: Boot Phases (sysinit, preconfigured, configured, ready, shutdown, final)

use crate::error::Result;
use crate::predict::{extend_pcr_string, PcrContext, PcrIndex, PcrRecord, PCR_INIT_VAL};

/// Systemd boot phase strings extended into PCR 11.
const PHASES: &[&str] = &[
    "sysinit",
    "preconfigured",
    "configured",
    "ready",
    "shutdown",
    "final",
];

/// Predict PCR 11 values for all boot phases.
pub fn predict(_ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    let mut digests = Vec::with_capacity(PHASES.len() + 1);
    let mut pcr = PCR_INIT_VAL;
    digests.push(pcr);
    for phase in PHASES {
        pcr = extend_pcr_string(&pcr, phase);
        digests.push(pcr);
    }
    Ok(Some((PcrIndex::Pcr11, PcrRecord::new_multi(digests))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;
    use crate::predict::test_support::MockCtx;
    use test_case::test_case;

    #[test]
    fn test_predict_count() {
        let m = MockCtx::new();
        let ctx = m.build(Platform::Aws);
        let result = predict(&ctx).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr11);
        assert_eq!(result.1.sha256.len(), 7);
    }

    #[test_case(0, "0000000000000000000000000000000000000000000000000000000000000000" ; "zero")]
    #[test_case(1, "02ab266cdc69ade4603be47fa9c95ae95c91d8c5b13c32bc4708b97d5ad0d3fe" ; "sysinit")]
    #[test_case(2, "9865b840fa2f504f9dbbcaa4e2380aac2a7e9bab057fd50e26e3f9eaa1d24551" ; "preconfigured")]
    #[test_case(3, "9a55d57ac0252cfd7176546314f97c6be58d29176601dea7daa03ca9fc5b5911" ; "configured")]
    #[test_case(4, "857516a0408cb6ef303e5e324be52f3860471420e32415506c8c6b8273a4fe23" ; "ready")]
    #[test_case(5, "93e1a6e06e8e8ef2399a75578126352d4bb7761e1c8caa143b61601f202b8675" ; "shutdown")]
    #[test_case(6, "5fd4d826d33984644d444224003f56dde24681ea74c2fb9fbc016d71cdb861b4" ; "final_phase")]
    fn test_predict_phase(index: usize, expected: &str) {
        let m = MockCtx::new();
        let ctx = m.build(Platform::Aws);
        let result = predict(&ctx).unwrap().unwrap();
        assert_eq!(result.1.sha256[index], expected);
    }
}
