//! PCR 5: GPT partition table measurements.
//!
//! Generates predictions covering all valid GPT priority bit combinations:
//! - Dual-bank: 6 BOOT-A states × 6 BOOT-B states × 2 PRIVATE states = 72 combinations
//! - Single-bank: 6 BOOT-A states × 2 PRIVATE states = 12 combinations
//!
//! Platform differences:
//! - AWS/Metal: separator -> GPT -> exit actions
//! - VMware: Not predicted (EV_EFI_VARIABLE_BOOT events for BootOrder, Boot0000, Boot0001
//!   contain VM-specific device paths with MAC addresses that vary per VM instance)

use crate::error::Result;
use crate::gpt::{GptPrio, Nibble, PartitionTable};
use crate::image::BootChain;
use crate::platform::Platform;
use crate::predict::{
    extend_pcr_data, extend_pcr_separator, extend_pcr_string, PcrContext, PcrDigests, PcrIndex,
    PcrRecord, PCR_INIT_VAL,
};
use snafu::{FromString, Whatever};

const ACTION_STRING_1: &str = "Exit Boot Services Invocation";
const ACTION_STRING_2: &str = "Exit Boot Services Returned with Success";

/// Priority bit combinations: (priority, tries_left, successful)
const COMBINATIONS: [(u8, u8, bool); 6] = [
    (0, 0, false), // inactive
    (0, 1, false), // inactive with tries
    (1, 0, true),  // priority 1, successful
    (2, 0, false), // priority 2, no tries
    (2, 0, true),  // priority 2, successful
    (2, 1, false), // priority 2, with tries
];

/// Measure PCR 5 for one GPT state: separator -> EFI_GPT_DATA -> exit actions.
fn measure(gpt: &PartitionTable, attributes: &[(u32, GptPrio)]) -> Result<PcrDigests> {
    let mut pcr = extend_pcr_separator(&PCR_INIT_VAL);
    pcr = extend_pcr_data(&pcr, &gpt.efi_gpt_data(attributes)?);
    pcr = extend_pcr_string(&pcr, ACTION_STRING_1);
    Ok(extend_pcr_string(&pcr, ACTION_STRING_2))
}

/// Boot partition attributes with gptprio bits 48-56 replaced.
fn with_priority(
    mut attrs: GptPrio,
    (priority, tries, successful): (u8, u8, bool),
) -> Result<GptPrio> {
    let nibble = |v| Nibble::try_new(v).map_err(|e| Whatever::without_source(e.to_string()));
    attrs.set_priority(nibble(priority)?);
    attrs.set_tries_left(nibble(tries)?);
    attrs.set_successful(successful);
    Ok(attrs)
}

/// Predict PCR 5 values for all GPT state combinations.
///
/// Returns `None` for VMware since boot variables contain VM-specific data.
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    if ctx.platform == Platform::Vmware {
        return Ok(None);
    }

    let gpt = &ctx.image.gpt;
    let partitions = match &ctx.image.boot {
        // Direct UKI boot has no BOOT partitions, so nothing rewrites GPT attributes.
        BootChain::Uki(_) => {
            return Ok(Some((PcrIndex::Pcr5, PcrRecord::new(measure(gpt, &[])?))));
        }
        BootChain::Grub(grub) => &grub.partitions,
    };

    let boot_b_combos: &[(u8, u8, bool)] = if partitions.dual_bank() {
        &COMBINATIONS
    } else {
        &[(0, 0, false)] // Single placeholder for single-bank
    };

    let capacity = COMBINATIONS.len() * boot_b_combos.len() * 2;
    let mut digests = Vec::with_capacity(capacity);

    for private_bit57 in [false, true] {
        let mut private = gpt.attributes(partitions.private);
        private.set_boot_succeeded(private_bit57);
        for a_state in COMBINATIONS {
            let boot_a = with_priority(gpt.attributes(partitions.boot_a), a_state)?;
            for &b_state in boot_b_combos {
                let mut attributes = vec![(partitions.boot_a, boot_a)];
                if let Some(boot_b) = partitions.boot_b {
                    attributes.push((boot_b, with_priority(gpt.attributes(boot_b), b_state)?));
                }
                attributes.push((partitions.private, private));
                digests.push(measure(gpt, &attributes)?);
            }
        }
    }

    Ok(Some((PcrIndex::Pcr5, PcrRecord::new_multi(digests))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{BootPartitions, DiskImage, Grub};
    use crate::predict::test_support::{
        mock_efi_vars, mock_gpt, mock_grub, mock_image, mock_uki, MockCtx,
    };
    use sha2::{Digest, Sha256};

    /// Digest of all predicted states, pinned to catch any change in the measured bytes.
    fn predict_digest(image: DiskImage) -> (usize, String) {
        let efi_vars = mock_efi_vars();
        let ctx = PcrContext {
            platform: Platform::Aws,
            efi_vars: &efi_vars,
            image: &image,
        };
        let (index, record) = predict(&ctx).unwrap().unwrap();
        assert_eq!(index, PcrIndex::Pcr5);
        assert_eq!(record.sha256.len(), record.sha384.len());
        let all = format!("{:?}{:?}", record.sha256, record.sha384);
        (record.sha256.len(), hex::encode(Sha256::digest(all)))
    }

    fn grub(boot_b: Option<u32>, private: u32) -> BootChain {
        BootChain::Grub(Box::new(Grub {
            partitions: BootPartitions {
                boot_a: 2,
                boot_b,
                private,
            },
            ..mock_grub()
        }))
    }

    #[test]
    fn test_dual_bank_aws() {
        let (states, digest) = predict_digest(mock_image(mock_gpt(4), grub(Some(3), 4)));
        assert_eq!(states, 72);
        assert_eq!(
            digest,
            "caf74fc8a9a1609063213501ab023ab6ca7546ca31c21bca7be8a666639e77f7"
        );
    }

    #[test]
    fn test_single_bank_aws() {
        let (states, digest) = predict_digest(mock_image(mock_gpt(3), grub(None, 3)));
        assert_eq!(states, 12);
        assert_eq!(
            digest,
            "038501c0cd266f011a8b6d6c9786c51165bebb4065e7952a34ff720cfbaa917d"
        );
    }

    #[test]
    fn test_uki_aws() {
        let image = mock_image(mock_gpt(1), BootChain::Uki(mock_uki(0x8664)));
        let efi_vars = mock_efi_vars();
        let ctx = PcrContext {
            platform: Platform::Aws,
            efi_vars: &efi_vars,
            image: &image,
        };
        let (_, record) = predict(&ctx).unwrap().unwrap();
        assert_eq!(
            record.sha256,
            ["19ffcbff829bc8cb486cd9bf351966f7592b0e8c4e0cc266c179b6218d2b3fe4"]
        );
    }

    #[test]
    fn test_vmware_not_predicted() {
        let m = MockCtx::new();
        assert!(predict(&m.build(Platform::Vmware)).unwrap().is_none());
    }
}
