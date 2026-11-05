//! PCR 4: Boot Manager Code (shim, grub, vmlinuz or UKI)

use crate::error::Result;
use crate::image::BootChain;
use crate::platform::Platform;
use crate::predict::{
    extend_pcr, extend_pcr_separator, extend_pcr_string, PcrContext, PcrIndex, PcrRecord,
    PCR_INIT_VAL,
};

const ACTION_STRING: &str = "Calling EFI Application from Boot Option";

/// Predict PCR 4 value.
///
/// AWS/Metal extend an action string before the separator, VMware does not.
///
/// AWS/Metal GRUB: action -> separator -> shim -> grub -> vmlinuz
/// AWS UKI:        action -> separator -> UKI
/// VMware GRUB:    separator -> shim -> grub -> vmlinuz
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    let images = match &ctx.image.boot {
        BootChain::Uki(uki) => vec![uki.authenticode()],
        BootChain::Grub(grub) if grub.partitions.dual_bank() => return Ok(None),
        BootChain::Grub(grub) => vec![
            grub.shim.authenticode(),
            grub.grub.authenticode(),
            grub.vmlinuz.authenticode(),
        ],
    };

    // AWS/Metal: action string first, VMware: start with zeros
    let mut pcr = match ctx.platform {
        Platform::Aws | Platform::Metal => extend_pcr_string(&PCR_INIT_VAL, ACTION_STRING),
        Platform::Vmware => PCR_INIT_VAL,
    };

    pcr = extend_pcr_separator(&pcr);
    for image in images {
        pcr = extend_pcr(&pcr, image);
    }

    Ok(Some((PcrIndex::Pcr4, PcrRecord::new(pcr))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::predict::test_support::{mock_uki, MockCtx};

    #[test]
    fn test_predict_aws() {
        let m = MockCtx::new();
        let result = predict(&m.build(Platform::Aws)).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr4);
        assert_eq!(
            result.1.sha256[0],
            "b60bad6ffbd166bbbfcc81fa7ccd9977fb751385bf93a3c735d4edf997839a72"
        );
    }

    #[test]
    fn test_predict_vmware() {
        let m = MockCtx::new();
        let result = predict(&m.build(Platform::Vmware)).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr4);
        assert_eq!(
            result.1.sha256[0],
            "566c80e14cd36caed8cb2c10b4298520f9d6c3e980fc09dcdde63d4613b6c4b5"
        );
    }

    #[test]
    fn test_predict_skipped_for_ab() {
        let m = MockCtx::dual_bank();
        assert!(predict(&m.build(Platform::Aws)).unwrap().is_none());
    }

    #[test]
    fn test_predict_uki_measures_the_whole_image() {
        let uki = mock_uki(0x8664);
        let pcr = extend_pcr_separator(&extend_pcr_string(&PCR_INIT_VAL, ACTION_STRING));
        let expected = extend_pcr(&pcr, uki.authenticode());
        let m = MockCtx::with_boot(BootChain::Uki(uki));
        let result = predict(&m.build(Platform::Aws)).unwrap().unwrap();
        assert_eq!(result.1, PcrRecord::new(expected));
    }
}
