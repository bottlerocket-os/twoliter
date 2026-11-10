//! Disk image layout and byte-range hashing.

use crate::error::Result;
use gptman::GPT;
use pcrsys::gpt::uuid_to_guid;
use sha2::{Digest, Sha256};
use snafu::ResultExt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// Bottlerocket data partition type GUID (626f7474-6c65-6474-6861-726d61726b73).
pub const DATA_TYPE_GUID: [u8; 16] = uuid_to_guid([
    0x62, 0x6f, 0x74, 0x74, 0x6c, 0x65, 0x64, 0x74, 0x68, 0x61, 0x72, 0x6d, 0x61, 0x72, 0x6b, 0x73,
]);

const BUF_SIZE: usize = 1 << 20;

/// A partition from the GPT.
#[derive(Debug, Clone)]
pub struct Partition {
    /// Short name such as `ROOT-A`, derived from the GPT label.
    pub name: String,
    pub offset: u64,
    pub len: u64,
}

/// GPT-derived layout of a disk image, in bytes.
#[derive(Debug, Clone)]
pub struct DiskLayout {
    /// End of the primary GPT (first usable byte).
    pub primary_gpt_end: u64,
    /// Start and end of the backup GPT (partition entries and header).
    pub backup_gpt: (u64, u64),
    pub partitions: Vec<Partition>,
}

impl DiskLayout {
    pub fn read(file: &mut File) -> Result<Self> {
        file.seek(SeekFrom::Start(0))
            .whatever_context("failed to seek to GPT")?;
        let gpt = GPT::find_from(file).whatever_context("failed to parse GPT")?;
        let ss = gpt.sector_size;
        let mut data_count = 0;
        let mut partitions: Vec<Partition> = gpt
            .iter()
            .filter(|(_, p)| p.is_used())
            .map(|(number, p)| {
                let label = p.partition_name.as_str().trim_end_matches('\0');
                let name = match label {
                    "EFI-SYSTEM" => "EFI-A".to_string(),
                    "EFI-BACKUP" => "EFI-B".to_string(),
                    "BIOS-BOOT" => "BIOS".to_string(),
                    l if l.starts_with("BOTTLEROCKET-") => l["BOTTLEROCKET-".len()..].to_string(),
                    _ if p.partition_type_guid == DATA_TYPE_GUID => {
                        data_count += 1;
                        if data_count == 1 { "DATA-A" } else { "DATA-B" }.to_string()
                    }
                    "" => format!("PARTITION-{number}"),
                    l => l.to_string(),
                };
                Partition {
                    name,
                    offset: p.starting_lba * ss,
                    len: (p.ending_lba - p.starting_lba + 1) * ss,
                }
            })
            .collect();
        partitions.sort_by_key(|p| p.offset);

        Ok(Self {
            primary_gpt_end: gpt.header.first_usable_lba * ss,
            backup_gpt: (
                (gpt.header.last_usable_lba + 1) * ss,
                (gpt.header.backup_lba + 1) * ss,
            ),
            partitions,
        })
    }

    pub fn find(&self, name: &str) -> Option<&Partition> {
        self.partitions.iter().find(|p| p.name == name)
    }

    /// (Offset, length) ranges covering the protective MBR and both GPTs.
    pub fn gpt_ranges(&self) -> [(u64, u64); 2] {
        [
            (0, self.primary_gpt_end),
            (self.backup_gpt.0, self.backup_gpt.1 - self.backup_gpt.0),
        ]
    }

    /// Byte ranges below `len` that are not covered by the MBR, either GPT, or any partition.
    pub fn unallocated(&self, len: u64) -> Vec<(u64, u64)> {
        let mut used = self.gpt_ranges().to_vec();
        used.extend(self.partitions.iter().map(|p| (p.offset, p.len)));
        used.sort();

        let mut gaps = Vec::new();
        let mut pos = 0;
        for (offset, size) in used {
            if offset > pos {
                gaps.push((pos, offset.min(len) - pos));
            }
            pos = pos.max(offset + size);
            if pos >= len {
                break;
            }
        }
        if pos < len {
            gaps.push((pos, len - pos));
        }
        gaps.retain(|(_, size)| *size > 0);
        gaps
    }
}

/// Digest of a set of byte ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeHash {
    pub sha256: [u8; 32],
    pub all_zero: bool,
}

impl RangeHash {
    pub fn hex(&self) -> String {
        hex::encode(self.sha256)
    }
}

/// Hash the concatenation of `ranges` (offset, length) in `file`.
pub fn hash_ranges(file: &mut File, ranges: &[(u64, u64)]) -> Result<RangeHash> {
    let mut hasher = Sha256::new();
    let mut all_zero = true;
    let mut buf = vec![0u8; BUF_SIZE];
    for &(offset, len) in ranges {
        file.seek(SeekFrom::Start(offset))
            .whatever_context(format!("failed to seek to {offset}"))?;
        let mut remaining = len;
        while remaining > 0 {
            let n = remaining.min(BUF_SIZE as u64) as usize;
            file.read_exact(&mut buf[..n])
                .whatever_context(format!("failed to read {len} bytes at {offset}"))?;
            hasher.update(&buf[..n]);
            all_zero = all_zero && buf[..n].iter().all(|b| *b == 0);
            remaining -= n as u64;
        }
    }
    Ok(RangeHash {
        sha256: hasher.finalize().into(),
        all_zero,
    })
}

pub fn file_len(file: &File) -> Result<u64> {
    Ok(file
        .metadata()
        .whatever_context("failed to stat image")?
        .len())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use gptman::GPTPartitionEntry;
    use std::io::Write;

    pub const MIB: u64 = 1 << 20;

    /// Write a GPT with the given (label, type GUID, start MiB, size MiB) partitions to `file`.
    pub fn write_gpt(file: &mut File, len: u64, parts: &[(&str, [u8; 16], u64, u64)]) {
        file.set_len(len).unwrap();
        let mut gpt = GPT::new_from(file, 512, [7u8; 16]).unwrap();
        for (i, (label, type_guid, start, size)) in parts.iter().enumerate() {
            gpt[i as u32 + 1] = GPTPartitionEntry {
                partition_type_guid: *type_guid,
                unique_partition_guid: [i as u8 + 1; 16],
                starting_lba: start * MIB / 512,
                ending_lba: (start + size) * MIB / 512 - 1,
                attribute_bits: 0,
                partition_name: (*label).into(),
            };
        }
        gpt.write_into(file).unwrap();
        GPT::write_protective_mbr_into(file, 512).unwrap();
        file.flush().unwrap();
    }

    #[test]
    fn reads_layout_and_gaps() {
        let mut f = tempfile::tempfile().unwrap();
        write_gpt(
            &mut f,
            8 * MIB,
            &[
                ("BOTTLEROCKET-ROOT-A", [1; 16], 1, 2),
                ("BOTTLEROCKET-ROOT-B", [1; 16], 4, 1),
                ("", DATA_TYPE_GUID, 5, 2),
            ],
        );
        let layout = DiskLayout::read(&mut f).unwrap();
        let names: Vec<_> = layout.partitions.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["ROOT-A", "ROOT-B", "DATA-A"]);
        assert_eq!(layout.find("ROOT-B").unwrap().offset, 4 * MIB);
        assert_eq!(layout.primary_gpt_end, 34 * 512);
        assert_eq!(layout.backup_gpt, (8 * MIB - 33 * 512, 8 * MIB));
        assert_eq!(
            layout.gpt_ranges(),
            [(0, 34 * 512), (8 * MIB - 33 * 512, 33 * 512)]
        );

        let gaps = layout.unallocated(8 * MIB);
        assert_eq!(
            gaps,
            vec![
                (34 * 512, MIB - 34 * 512),
                (3 * MIB, MIB),
                (7 * MIB, MIB - 33 * 512)
            ]
        );
    }

    #[test]
    fn hashes_ranges() {
        let mut f = tempfile::tempfile().unwrap();
        f.write_all(&[0, 0, 1, 0]).unwrap();
        assert!(hash_ranges(&mut f, &[(0, 2)]).unwrap().all_zero);
        assert!(!hash_ranges(&mut f, &[(0, 4)]).unwrap().all_zero);
        assert_eq!(
            hash_ranges(&mut f, &[(0, 1), (2, 2)]).unwrap(),
            hash_ranges(&mut f, &[(1, 3)]).unwrap()
        );
        assert!(hash_ranges(&mut f, &[(0, 5)]).is_err());
    }
}
