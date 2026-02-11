//! PCR 4: Boot Manager Code (shim, grub, vmlinuz)

use crate::error::Result;
use crate::pe::get_authenticode_hash;
use crate::platform::Platform;
use crate::predict::{
    extend_pcr, extend_pcr_separator, extend_pcr_string, PcrContext, PcrIndex, PcrRecord,
    PCR_INIT_VAL,
};

/// Predict PCR 4 value.
///
/// AWS/Metal extend an action string before the separator, VMware does not.
///
/// AWS/Metal: action -> separator -> shim -> grub -> vmlinuz
/// VMware:              separator -> shim -> grub -> vmlinuz
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    if ctx.partitions.boot_b.is_some() {
        return Ok(None);
    }

    let shim_hash = get_authenticode_hash(ctx.shim)?;
    let grub_hash = get_authenticode_hash(ctx.grub)?;
    let vmlinuz_hash = get_authenticode_hash(ctx.vmlinuz)?;

    // AWS/Metal: action string first, VMware: start with zeros
    let mut pcr = match ctx.platform {
        Platform::Aws | Platform::Metal => {
            extend_pcr_string(&PCR_INIT_VAL, "Calling EFI Application from Boot Option")
        }
        Platform::Vmware => PCR_INIT_VAL,
    };

    // Common: separator -> shim -> grub -> vmlinuz
    pcr = extend_pcr_separator(&pcr);
    pcr = extend_pcr(&pcr, &shim_hash);
    pcr = extend_pcr(&pcr, &grub_hash);
    pcr = extend_pcr(&pcr, &vmlinuz_hash);

    Ok(Some((PcrIndex::Pcr4, PcrRecord::new(pcr))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::predict::test_support::{build_test_shim, MockCtx};

    #[test]
    fn test_predict_aws() {
        let pe = build_test_shim();
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(Platform::Aws)
            .efi_vars(&m.efi_vars)
            .partitions(&m.layout)
            .shim(&pe)
            .grub(&pe)
            .vmlinuz(&pe)
            .build();
        let result = predict(&ctx).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr4);
        assert_eq!(
            result.1.sha256[0],
            "b60bad6ffbd166bbbfcc81fa7ccd9977fb751385bf93a3c735d4edf997839a72"
        );
    }

    #[test]
    fn test_predict_vmware() {
        let pe = build_test_shim();
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(Platform::Vmware)
            .efi_vars(&m.efi_vars)
            .partitions(&m.layout)
            .shim(&pe)
            .grub(&pe)
            .vmlinuz(&pe)
            .build();
        let result = predict(&ctx).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr4);
        assert_eq!(
            result.1.sha256[0],
            "566c80e14cd36caed8cb2c10b4298520f9d6c3e980fc09dcdde63d4613b6c4b5"
        );
    }

    #[test]
    fn test_predict_skipped_for_ab() {
        let pe = build_test_shim();
        let m = MockCtx::dual_bank();
        let ctx = PcrContext::builder()
            .platform(Platform::Aws)
            .efi_vars(&m.efi_vars)
            .partitions(&m.layout)
            .shim(&pe)
            .grub(&pe)
            .vmlinuz(&pe)
            .build();
        assert!(predict(&ctx).unwrap().is_none());
    }
}
