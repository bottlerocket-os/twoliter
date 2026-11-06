//! Core PCR prediction types and functions.

use serde::Serialize;

use std::collections::BTreeMap;

use sha2::{Digest, Sha256, Sha384};

use crate::efi::EfiVars;
use crate::image::DiskImage;
use crate::platform::Platform;

use crate::error::Result;

/// One digest per predicted TPM bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcrDigests {
    pub sha256: [u8; 32],
    pub sha384: [u8; 48],
}

impl PcrDigests {
    pub fn hash(data: &[u8]) -> Self {
        Self {
            sha256: Sha256::digest(data).into(),
            sha384: Sha384::digest(data).into(),
        }
    }
}

/// Initial PCR values: one zero-filled digest per bank.
pub const PCR_INIT_VAL: PcrDigests = PcrDigests {
    sha256: [0; 32],
    sha384: [0; 48],
};

/// TPM Platform Configuration Register index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum PcrIndex {
    Pcr0 = 0,
    Pcr1 = 1,
    Pcr2 = 2,
    Pcr3 = 3,
    Pcr4 = 4,
    Pcr5 = 5,
    Pcr6 = 6,
    Pcr7 = 7,
    #[allow(dead_code)]
    Pcr8 = 8,
    Pcr9 = 9,
    Pcr10 = 10,
    Pcr11 = 11,
    Pcr12 = 12,
    Pcr13 = 13,
    Pcr14 = 14,
    Pcr15 = 15,
}

impl Serialize for PcrIndex {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_u8(*self as u8)
    }
}

/// Everything a PCR prediction reads.
pub struct PcrContext<'a> {
    pub platform: Platform,
    pub efi_vars: &'a EfiVars,
    pub image: &'a DiskImage,
}

/// Collection of PCR predictions for JSON output.
#[derive(Default, Serialize)]
pub struct PcrPredictions {
    /// Map of PCR index to record.
    pub pcrs: BTreeMap<PcrIndex, PcrRecord>,
}

impl PcrPredictions {
    /// Create a new empty set of PCR predictions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Try a prediction function; insert result if Some, propagate errors.
    ///
    /// This enables fluent chaining of PCR predictions.
    pub fn try_extend(
        mut self,
        f: impl FnOnce() -> Result<Option<(PcrIndex, PcrRecord)>>,
    ) -> Result<Self> {
        if let Some((index, record)) = f()? {
            self.pcrs.insert(index, record);
        }
        Ok(self)
    }
}

/// A single PCR prediction record.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PcrRecord {
    /// SHA-256 digest values for this PCR.
    pub sha256: Vec<String>,
    /// SHA-384 digest values in the same state order as SHA-256.
    pub sha384: Vec<String>,
}

impl PcrRecord {
    /// Create a new PCR record with one state for both banks.
    pub fn new(digest: PcrDigests) -> Self {
        Self::new_multi(vec![digest])
    }

    /// Create a new PCR record with multiple states for both banks.
    pub fn new_multi(digests: Vec<PcrDigests>) -> Self {
        let (sha256, sha384) = digests
            .into_iter()
            .map(|digest| (hex::encode(digest.sha256), hex::encode(digest.sha384)))
            .unzip();
        Self { sha256, sha384 }
    }
}

/// Extend each bank with its own event digest: `H(current || digest)`.
pub fn extend_pcr(current: &PcrDigests, digest: &PcrDigests) -> PcrDigests {
    PcrDigests {
        sha256: Sha256::new()
            .chain_update(current.sha256)
            .chain_update(digest.sha256)
            .finalize()
            .into(),
        sha384: Sha384::new()
            .chain_update(current.sha384)
            .chain_update(digest.sha384)
            .finalize()
            .into(),
    }
}

/// Extend a PCR with an event payload by first hashing its bytes.
pub fn extend_pcr_data(current: &PcrDigests, data: &[u8]) -> PcrDigests {
    extend_pcr(current, &PcrDigests::hash(data))
}

/// Extend a PCR with a string by first hashing the string.
pub fn extend_pcr_string(current: &PcrDigests, s: &str) -> PcrDigests {
    extend_pcr_data(current, s.as_bytes())
}

/// Extend a PCR with the EV_SEPARATOR event (4 zero bytes).
pub fn extend_pcr_separator(current: &PcrDigests) -> PcrDigests {
    extend_pcr_data(current, &[0; 4])
}

#[cfg(test)]
pub mod test_support {
    use super::PcrContext;
    use crate::efi::EfiVars;
    use crate::gpt::PartitionTable;
    use crate::image::{BootChain, BootPartitions, DiskImage, Grub};
    use crate::pe::{PeImage, Shim, Uki};
    use crate::platform::Platform;
    use bootconfig::{Bootconfig, GrubCmdline};
    use gptman::{GPTPartitionEntry, GPT};

    pub(crate) use crate::pe::tests::{build_test_pe, build_test_shim, build_test_uki};

    /// SHA-256 of extending PCR init value with the EV_SEPARATOR event.
    pub const SEPARATOR_HASH: &str =
        "3d458cfe55cc03ea1f443f1562beec8df51c75e14a9fcf9a7234a13f198e7969";

    /// Create empty EFI variables for testing.
    pub fn mock_efi_vars() -> EfiVars {
        EfiVars { variables: vec![] }
    }

    /// Create a GPT with `parts` used partitions, each with distinct GUIDs, attributes, and names.
    pub fn mock_gpt(parts: u32) -> GPT {
        let mut disk = std::io::Cursor::new(vec![0u8; 128 * 512]);
        let mut gpt = GPT::new_from(&mut disk, 512, [0xaa; 16]).unwrap();
        for i in 1..=parts {
            let start_lba = gpt.header.first_usable_lba + u64::from(i - 1) * 4;
            gpt[i] = GPTPartitionEntry {
                partition_type_guid: [i as u8; 16],
                unique_partition_guid: [0x10 + i as u8; 16],
                starting_lba: start_lba,
                ending_lba: start_lba + 3,
                attribute_bits: 1 << (i - 1),
                partition_name: format!("PART-{i}").as_str().into(),
            };
        }
        gpt
    }

    /// Single-bank GRUB chain (BOOT-A 2, PRIVATE 3) with the test shim for every executable.
    pub fn mock_grub() -> Grub {
        let pe = PeImage::parse(&build_test_shim()).unwrap();
        Grub {
            shim: Shim::from_pe(pe.clone()).unwrap(),
            grub: pe.clone(),
            vmlinuz: pe,
            cmdline: GrubCmdline::parse(b"linux ($root)/vmlinuz", "").unwrap(),
            bootconfig: Bootconfig::default(),
            partitions: BootPartitions {
                boot_a: 2,
                boot_b: None,
                private: 3,
            },
        }
    }

    pub fn mock_uki(machine: u16) -> Uki {
        Uki::from_pe(PeImage::parse(&build_test_uki(machine)).unwrap())
            .unwrap()
            .unwrap()
    }

    pub fn mock_image(gpt: GPT, boot: BootChain) -> DiskImage {
        DiskImage {
            gpt: PartitionTable::new(gpt).unwrap(),
            boot,
        }
    }

    /// Test context that owns EfiVars and DiskImage for convenient test setup.
    pub struct MockCtx {
        pub efi_vars: EfiVars,
        pub image: DiskImage,
    }

    impl MockCtx {
        /// Single-bank GRUB image.
        pub fn new() -> Self {
            Self::with_boot(BootChain::Grub(Box::new(mock_grub())))
        }

        /// Dual-bank GRUB image (BOOT-A 2, BOOT-B 4, PRIVATE 3).
        pub fn dual_bank() -> Self {
            let mut grub = mock_grub();
            grub.partitions.boot_b = Some(4);
            Self::with_boot(BootChain::Grub(Box::new(grub)))
        }

        pub fn with_boot(boot: BootChain) -> Self {
            Self {
                efi_vars: mock_efi_vars(),
                image: mock_image(mock_gpt(4), boot),
            }
        }

        /// Build a PcrContext with the given platform.
        pub fn build(&self, platform: Platform) -> PcrContext<'_> {
            PcrContext {
                platform,
                efi_vars: &self.efi_vars,
                image: &self.image,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_extends_each_bank_at_its_own_width() {
        // Expected values from Python hashlib: H(zero_digest || H(b"test")).
        let result = extend_pcr_data(&PCR_INIT_VAL, b"test");
        assert_eq!(
            hex::encode(result.sha256),
            "516caf854bba78a30ba2a84f9e400642c01c1a3fa429268ff5b47c32a655d4b3"
        );
        assert_eq!(
            hex::encode(result.sha384),
            "e6c7526759cbdca9a11ba0bf7efe6d2193308532a85ba7e969889200de19583f4d3746983b22bf4638d571ec8aeabb85"
        );
    }

    #[test]
    fn records_serialize_both_banks_in_state_order() {
        let record = PcrRecord::new_multi(vec![
            PcrDigests {
                sha256: [0xab; 32],
                sha384: [0xcd; 48],
            },
            PcrDigests {
                sha256: [0xef; 32],
                sha384: [0x12; 48],
            },
        ]);
        assert_eq!(
            serde_json::to_value(record).unwrap(),
            serde_json::json!({
                "sha256": ["ab".repeat(32), "ef".repeat(32)],
                "sha384": ["cd".repeat(48), "12".repeat(48)],
            })
        );
    }
}
