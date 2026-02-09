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
use crate::gpt::{
    build_efi_gpt_data, recalculate_gpt_crcs, set_gpt_priority_bits, set_private_succeeded,
};
use crate::platform::Platform;
use crate::predict::{
    extend_pcr, extend_pcr_separator, extend_pcr_string, PcrContext, PcrIndex, PcrRecord,
    PCR_INIT_VAL,
};
use sha2::{Digest, Sha256};

/// Priority bit combinations: (priority, tries_left, successful)
const COMBINATIONS: [(u8, u8, bool); 6] = [
    (0, 0, false), // inactive
    (0, 1, false), // inactive with tries
    (1, 0, true),  // priority 1, successful
    (2, 0, false), // priority 2, no tries
    (2, 0, true),  // priority 2, successful
    (2, 1, false), // priority 2, with tries
];

/// Predict PCR 5 values for all GPT state combinations.
///
/// Returns `None` for VMware since boot variables contain VM-specific data.
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    if ctx.platform == Platform::Vmware {
        return Ok(None);
    }

    let boot_b_combos: &[(u8, u8, bool)] = if ctx.partitions.boot_b.is_some() {
        &COMBINATIONS
    } else {
        &[(0, 0, false)] // Single placeholder for single-bank
    };

    let capacity = COMBINATIONS.len() * boot_b_combos.len() * 2;
    let mut digests = Vec::with_capacity(capacity);

    for private_bit57 in [false, true] {
        for (a_prio, a_tries, a_succ) in COMBINATIONS {
            for (b_prio, b_tries, b_succ) in boot_b_combos {
                let mut gpt_copy = ctx.gpt_bin.to_vec();

                // Set BOOT-A priority bits
                set_gpt_priority_bits(
                    &mut gpt_copy,
                    ctx.partitions.boot_a.number,
                    a_prio,
                    a_tries,
                    a_succ,
                )?;

                // Set BOOT-B priority bits if present
                if let Some(boot_b) = &ctx.partitions.boot_b {
                    set_gpt_priority_bits(
                        &mut gpt_copy,
                        boot_b.number,
                        *b_prio,
                        *b_tries,
                        *b_succ,
                    )?;
                }

                // Set PRIVATE bit 57 explicitly (clear or set based on iteration)
                set_private_succeeded(&mut gpt_copy, ctx.partitions.private.number, private_bit57);

                // Recalculate CRCs after all modifications
                recalculate_gpt_crcs(&mut gpt_copy);

                // Build EFI_GPT_DATA and hash it
                let gpt_data = build_efi_gpt_data(&gpt_copy)?;
                let gpt_data_digest: [u8; 32] = Sha256::digest(&gpt_data).into();

                // PCR 5 = separator -> GPT_DATA -> exit actions
                let mut pcr = extend_pcr_separator(&PCR_INIT_VAL);
                pcr = extend_pcr(&pcr, &gpt_data_digest);
                pcr = extend_pcr_string(&pcr, "Exit Boot Services Invocation");
                pcr = extend_pcr_string(&pcr, "Exit Boot Services Returned with Success");

                digests.push(pcr);
            }
        }
    }

    Ok(Some((PcrIndex::Pcr5, PcrRecord::new_multi(digests))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpt::PartitionInfo;
    use crate::gpt::PartitionLayout;
    use crate::predict::test_support::MockCtx;

    // GPT header field offsets
    const GPT_SIGNATURE: std::ops::Range<usize> = 0..8;
    const GPT_REVISION: std::ops::Range<usize> = 8..12;
    const GPT_HEADER_SIZE: std::ops::Range<usize> = 12..16;
    const GPT_PARTITION_ENTRY_LBA: std::ops::Range<usize> = 72..80;
    const GPT_NUM_PARTITION_ENTRIES: std::ops::Range<usize> = 80..84;
    const GPT_PARTITION_ENTRY_SIZE: std::ops::Range<usize> = 84..88;
    const GPT_PARTITION_ARRAY_START: usize = 512;
    const GPT_PARTITION_ENTRY_LEN: usize = 128;

    /// Create mock GPT for dual-bank image (EFI-A, BOOT-A, BOOT-B, PRIVATE).
    fn mock_gpt_dual_bank() -> (Vec<u8>, PartitionLayout) {
        let mut gpt = vec![0u8; GPT_PARTITION_ARRAY_START + GPT_PARTITION_ENTRY_LEN * 128];
        gpt[GPT_SIGNATURE].copy_from_slice(b"EFI PART");
        gpt[GPT_REVISION].copy_from_slice(&0x00010000u32.to_le_bytes());
        gpt[GPT_HEADER_SIZE].copy_from_slice(&92u32.to_le_bytes());
        gpt[GPT_PARTITION_ENTRY_LBA].copy_from_slice(&2u64.to_le_bytes());
        gpt[GPT_NUM_PARTITION_ENTRIES].copy_from_slice(&128u32.to_le_bytes());
        gpt[GPT_PARTITION_ENTRY_SIZE].copy_from_slice(&128u32.to_le_bytes());
        for i in 0..4 {
            gpt[GPT_PARTITION_ARRAY_START + i * GPT_PARTITION_ENTRY_LEN] = (i + 1) as u8;
        }
        let layout = PartitionLayout {
            efi_a: PartitionInfo {
                number: 1,
                start_lba: 2048,
                end_lba: 4095,
            },
            boot_a: PartitionInfo {
                number: 2,
                start_lba: 4096,
                end_lba: 8191,
            },
            boot_b: Some(PartitionInfo {
                number: 3,
                start_lba: 8192,
                end_lba: 12287,
            }),
            private: PartitionInfo {
                number: 4,
                start_lba: 12288,
                end_lba: 16383,
            },
        };
        (gpt, layout)
    }

    /// Create mock GPT for single-bank image (EFI-A, BOOT-A, PRIVATE - no BOOT-B).
    fn mock_gpt_single_bank() -> (Vec<u8>, PartitionLayout) {
        let mut gpt = vec![0u8; GPT_PARTITION_ARRAY_START + GPT_PARTITION_ENTRY_LEN * 128];
        gpt[GPT_SIGNATURE].copy_from_slice(b"EFI PART");
        gpt[GPT_REVISION].copy_from_slice(&0x00010000u32.to_le_bytes());
        gpt[GPT_HEADER_SIZE].copy_from_slice(&92u32.to_le_bytes());
        gpt[GPT_PARTITION_ENTRY_LBA].copy_from_slice(&2u64.to_le_bytes());
        gpt[GPT_NUM_PARTITION_ENTRIES].copy_from_slice(&128u32.to_le_bytes());
        gpt[GPT_PARTITION_ENTRY_SIZE].copy_from_slice(&128u32.to_le_bytes());
        for i in 0..3 {
            gpt[GPT_PARTITION_ARRAY_START + i * GPT_PARTITION_ENTRY_LEN] = (i + 1) as u8;
        }
        let layout = PartitionLayout {
            efi_a: PartitionInfo {
                number: 1,
                start_lba: 2048,
                end_lba: 4095,
            },
            boot_a: PartitionInfo {
                number: 2,
                start_lba: 4096,
                end_lba: 8191,
            },
            boot_b: None,
            private: PartitionInfo {
                number: 3,
                start_lba: 8192,
                end_lba: 12287,
            },
        };
        (gpt, layout)
    }

    #[test]
    fn test_dual_bank_aws() {
        let (gpt, layout) = mock_gpt_dual_bank();
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(Platform::Aws)
            .efi_vars(&m.efi_vars)
            .partitions(&layout)
            .gpt_bin(&gpt)
            .build();
        let result = predict(&ctx).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr5);
        assert_eq!(result.1.sha256.len(), 72);
    }

    #[test]
    fn test_single_bank_aws() {
        let (gpt, layout) = mock_gpt_single_bank();
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(Platform::Aws)
            .efi_vars(&m.efi_vars)
            .partitions(&layout)
            .gpt_bin(&gpt)
            .build();
        let result = predict(&ctx).unwrap().unwrap();
        assert_eq!(result.0, PcrIndex::Pcr5);
        assert_eq!(result.1.sha256.len(), 12);
    }

    #[test]
    fn test_vmware_not_predicted() {
        let (gpt, layout) = mock_gpt_single_bank();
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(Platform::Vmware)
            .efi_vars(&m.efi_vars)
            .partitions(&layout)
            .gpt_bin(&gpt)
            .build();
        assert!(predict(&ctx).unwrap().is_none());
    }
}
