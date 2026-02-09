//! GPT structures and functions for PCR predictions.

use bit_field::BitField;
use gptman::GPT;
use hex_literal::hex;
use nutype::nutype;
use snafu::{FromString, OptionExt, ResultExt, Whatever};
use std::io::{Read, Seek, SeekFrom};
use uuid::Uuid;

use crate::error::Result;

/// 4-bit value (0-15) for GPT priority/tries fields.
#[nutype(validate(less_or_equal = 15), derive(Debug, Clone, Copy))]
pub struct Nibble(u8);

/// Convert UUID byte order to GPT GUID byte order.
///
/// GUIDs store the first three fields as little-endian, while UUIDs are big-endian.
pub const fn uuid_to_guid(uuid: [u8; 16]) -> [u8; 16] {
    [
        uuid[3], uuid[2], uuid[1], uuid[0], uuid[5], uuid[4], uuid[7], uuid[6], uuid[8], uuid[9],
        uuid[10], uuid[11], uuid[12], uuid[13], uuid[14], uuid[15],
    ]
}

/// GPT priority bits wrapper for boot partition attributes.
#[derive(Debug, Clone, Copy)]
pub struct GptPrio(u64);

impl GptPrio {
    /// Get priority value (bits 48-51).
    pub fn priority(self) -> Nibble {
        Nibble::try_new(self.0.get_bits(48..52) as u8).expect("4-bit field")
    }

    /// Set priority value (bits 48-51).
    pub fn set_priority(&mut self, priority: Nibble) {
        self.0.set_bits(48..52, priority.into_inner() as u64);
    }

    /// Get tries remaining (bits 52-55).
    pub fn tries_left(self) -> Nibble {
        Nibble::try_new(self.0.get_bits(52..56) as u8).expect("4-bit field")
    }

    /// Set tries remaining (bits 52-55).
    pub fn set_tries_left(&mut self, tries_left: Nibble) {
        self.0.set_bits(52..56, tries_left.into_inner() as u64);
    }

    /// Get successful boot flag (bit 56).
    pub fn successful(self) -> bool {
        self.0.get_bit(56)
    }

    /// Set successful boot flag (bit 56).
    pub fn set_successful(&mut self, successful: bool) {
        self.0.set_bit(56, successful);
    }

    /// Get boot-has-succeeded flag (bit 57).
    pub fn has_boot_succeeded(self) -> bool {
        self.0.get_bit(57)
    }

    /// Set boot-has-succeeded flag (bit 57).
    pub fn set_boot_succeeded(&mut self, succeeded: bool) {
        self.0.set_bit(57, succeeded);
    }
}

impl From<u64> for GptPrio {
    /// Create a `GptPrio` from raw partition attribute flags.
    fn from(flags: u64) -> Self {
        Self(flags)
    }
}

impl From<GptPrio> for u64 {
    /// Extract raw partition attribute flags from a `GptPrio`.
    fn from(flags: GptPrio) -> Self {
        flags.0
    }
}

/// Information about a single GPT partition.
///
/// Contains the partition number and LBA range for calculating byte offsets.
#[derive(Debug)]
pub struct PartitionInfo {
    /// 1-based partition number.
    pub number: u32,
    /// Starting LBA (512-byte sectors).
    pub start_lba: u64,
    /// Ending LBA (inclusive).
    pub end_lba: u64,
}

impl PartitionInfo {
    /// Calculate partition offset in bytes.
    pub fn offset_bytes(&self) -> u64 {
        self.start_lba * 512
    }

    /// Calculate partition size in bytes.
    pub fn size_bytes(&self) -> u64 {
        (self.end_lba - self.start_lba + 1) * 512
    }
}

/// Layout of partitions needed for PCR predictions.
///
/// Contains references to the EFI System Partition, boot partitions, and private partition.
/// Single-bank images will have `boot_b` set to `None`.
#[derive(Debug)]
pub struct PartitionLayout {
    /// EFI System Partition (ESP) - contains shim and grub.
    pub efi_a: PartitionInfo,
    /// Boot partition A - contains vmlinuz.
    pub boot_a: PartitionInfo,
    /// Boot partition B - alternate boot partition (None for single-bank images).
    pub boot_b: Option<PartitionInfo>,
    /// Private partition - contains bootconfig.
    pub private: PartitionInfo,
}

/// Extract primary GPT from disk image.
///
/// Returns LBA 1-33 (16896 bytes) containing GPT header and partition entries.
pub fn extract_primary_gpt<R: Read + Seek>(disk: &mut R) -> Result<Vec<u8>> {
    let start = 512u64; // LBA 1
    let len = 33 * 512; // 33 sectors

    disk.seek(SeekFrom::Start(start))
        .whatever_context("failed to seek to GPT")?;

    let mut buf = vec![0u8; len];
    disk.read_exact(&mut buf)
        .whatever_context("failed to read GPT")?;

    Ok(buf)
}

/// Bottlerocket boot partition type GUID.
const BOTTLEROCKET_BOOT: [u8; 16] = uuid_to_guid(hex!("6b636168 7420 6568 2070 6c616e657421"));

/// Bottlerocket private partition type GUID.
const BOTTLEROCKET_PRIVATE: [u8; 16] = uuid_to_guid(hex!("440408bb eb0b 4328 a6e5 a29038fad706"));

/// EFI System Partition type GUID.
const EFI_SYSTEM_PARTITION: [u8; 16] = uuid_to_guid(hex!("c12a7328 f81f 11d2 ba4b 00a0c93ec93b"));

/// Get the unique GUID of the first boot partition (BOOT-A).
pub fn get_boot_partuuid<R: Read + Seek>(disk: &mut R) -> Result<String> {
    let gpt = GPT::find_from(disk).whatever_context("failed to parse GPT")?;
    let (_, part) = gpt
        .iter()
        .find(|(_, p)| p.partition_type_guid == BOTTLEROCKET_BOOT)
        .whatever_context("BOOT-A partition not found")?;
    Ok(Uuid::from_bytes_le(part.unique_partition_guid).to_string())
}

/// Find partitions by type GUID and return their layout.
///
/// Parses GPT to find EFI-A, BOOT-A, BOOT-B (optional), and PRIVATE partitions.
/// Single-bank images will not have BOOT-B.
pub fn find_partitions<R: Read + Seek>(disk: &mut R) -> Result<PartitionLayout> {
    let gpt = GPT::find_from(disk).whatever_context("failed to parse GPT")?;

    // Find nth partition matching a type GUID
    let find_nth = |guid: &[u8; 16], n: usize| -> Option<PartitionInfo> {
        gpt.iter()
            .filter(|(_, p)| &p.partition_type_guid == guid)
            .nth(n)
            .map(|(num, p)| PartitionInfo {
                number: num,
                start_lba: p.starting_lba,
                end_lba: p.ending_lba,
            })
    };

    let efi_a = find_nth(&EFI_SYSTEM_PARTITION, 0).whatever_context("EFI-A partition not found")?;
    let boot_a = find_nth(&BOTTLEROCKET_BOOT, 0).whatever_context("BOOT-A partition not found")?;
    let boot_b = find_nth(&BOTTLEROCKET_BOOT, 1); // Optional for single-bank
    let private =
        find_nth(&BOTTLEROCKET_PRIVATE, 0).whatever_context("PRIVATE partition not found")?;

    Ok(PartitionLayout {
        efi_a,
        boot_a,
        boot_b,
        private,
    })
}

/// Build EFI_GPT_DATA structure as measured by EDK2 firmware.
///
/// Contains: GPT header (92 bytes) + partition count (u64 LE) + valid partition entries.
pub fn build_efi_gpt_data(gpt: &[u8]) -> Result<Vec<u8>> {
    // GPT header is first 92 bytes (at offset 0 in our extracted GPT which starts at LBA 1)
    let header = &gpt[0..92];

    // Partition entries start at offset 512 (LBA 2 relative to our extracted data)
    let entries_start = 512;
    let entry_size = 128;
    let max_entries = 128;

    // Count valid partitions (non-zero type GUID)
    let mut valid_entries = Vec::new();
    for i in 0..max_entries {
        let entry_offset = entries_start + i * entry_size;
        if entry_offset + entry_size > gpt.len() {
            break;
        }
        let entry = &gpt[entry_offset..entry_offset + entry_size];
        // Check if type GUID is non-zero
        if entry[0..16].iter().any(|&b| b != 0) {
            valid_entries.push(entry);
        }
    }

    let mut result = Vec::new();
    result.extend_from_slice(header);
    result.extend_from_slice(&(valid_entries.len() as u64).to_le_bytes());
    for entry in valid_entries {
        result.extend_from_slice(entry);
    }

    Ok(result)
}

/// Set GPT priority bits for a boot partition.
///
/// Modifies partition entry attributes at bits 48-56.
pub fn set_gpt_priority_bits(
    gpt: &mut [u8],
    part_num: u32,
    priority: u8,
    tries: u8,
    successful: bool,
) -> Result<()> {
    let entry_offset = 512 + (part_num - 1) as usize * 128;
    let attr_offset = entry_offset + 48;

    let priority =
        Nibble::try_new(priority).map_err(|e| Whatever::without_source(e.to_string()))?;
    let tries = Nibble::try_new(tries).map_err(|e| Whatever::without_source(e.to_string()))?;

    let mut attrs = u64::from_le_bytes(gpt[attr_offset..attr_offset + 8].try_into().unwrap());
    let mut prio = GptPrio::from(attrs);
    prio.set_priority(priority);
    prio.set_tries_left(tries);
    prio.set_successful(successful);
    attrs = prio.into();
    gpt[attr_offset..attr_offset + 8].copy_from_slice(&attrs.to_le_bytes());
    Ok(())
}

/// Set PRIVATE partition bit 57 (boot has ever succeeded).
pub fn set_private_succeeded(gpt: &mut [u8], part_num: u32, succeeded: bool) {
    let entry_offset = 512 + (part_num - 1) as usize * 128;
    let attr_offset = entry_offset + 48;

    let mut attrs = u64::from_le_bytes(gpt[attr_offset..attr_offset + 8].try_into().unwrap());
    let mut prio = GptPrio::from(attrs);
    prio.set_boot_succeeded(succeeded);
    attrs = prio.into();
    gpt[attr_offset..attr_offset + 8].copy_from_slice(&attrs.to_le_bytes());
}

/// Recalculate GPT CRCs after modifications.
///
/// Updates both the partition array CRC and header CRC.
pub fn recalculate_gpt_crcs(gpt: &mut [u8]) {
    // Zero header CRC field (offset 16, 4 bytes)
    gpt[16..20].copy_from_slice(&[0u8; 4]);

    // Calculate CRC32 of partition array (offset 512, 16384 bytes = 128 entries * 128 bytes)
    let partition_crc = crc32fast::hash(&gpt[512..512 + 16384]);
    gpt[88..92].copy_from_slice(&partition_crc.to_le_bytes());

    // Calculate CRC32 of header (first 92 bytes)
    let header_crc = crc32fast::hash(&gpt[0..92]);
    gpt[16..20].copy_from_slice(&header_crc.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_uuid_to_guid() {
        // Test from signpost: "Hah!IdontNeedEFI" BIOS boot partition GUID
        assert_eq!(
            uuid_to_guid(hex!("21686148 6449 6e6f 744e 656564454649")),
            *b"Hah!IdontNeedEFI"
        );
    }

    #[test]
    fn test_gpt_prio() {
        let mut prio = GptPrio::from(0u64);
        prio.set_priority(Nibble::try_new(2).unwrap());
        prio.set_tries_left(Nibble::try_new(3).unwrap());
        prio.set_successful(true);

        assert_eq!(prio.priority().into_inner(), 2);
        assert_eq!(prio.tries_left().into_inner(), 3);
        assert!(prio.successful());
        assert!(!prio.has_boot_succeeded());

        prio.set_boot_succeeded(true);
        assert!(prio.has_boot_succeeded());
    }

    #[test]
    fn test_partition_info_offset_bytes() {
        let info = PartitionInfo {
            number: 1,
            start_lba: 2048,
            end_lba: 4095,
        };
        assert_eq!(info.offset_bytes(), 2048 * 512);
    }

    #[test]
    fn test_partition_info_size_bytes() {
        let info = PartitionInfo {
            number: 1,
            start_lba: 2048,
            end_lba: 4095,
        };
        assert_eq!(info.size_bytes(), (4095 - 2048 + 1) * 512);
    }

    /// Create a minimal valid GPT for testing.
    fn mock_gpt() -> Vec<u8> {
        let mut gpt = vec![0u8; 512 + 128 * 128]; // header + entries

        // GPT header signature
        gpt[0..8].copy_from_slice(b"EFI PART");
        gpt[8..12].copy_from_slice(&0x00010000u32.to_le_bytes()); // Revision
        gpt[12..16].copy_from_slice(&92u32.to_le_bytes()); // Header size
        gpt[72..80].copy_from_slice(&2u64.to_le_bytes()); // Partition entry LBA
        gpt[80..84].copy_from_slice(&128u32.to_le_bytes()); // Number of entries
        gpt[84..88].copy_from_slice(&128u32.to_le_bytes()); // Entry size

        // Add 2 partitions with non-zero type GUIDs
        gpt[512] = 0x01; // Partition 1 type GUID byte
        gpt[512 + 128] = 0x02; // Partition 2 type GUID byte

        gpt
    }

    #[test]
    fn test_build_efi_gpt_data() {
        let gpt = mock_gpt();
        let data = build_efi_gpt_data(&gpt).unwrap();
        // 92 (header) + 8 (count) + 2*128 (2 valid entries)
        assert_eq!(data.len(), 92 + 8 + 256);
        // Check partition count
        let count = u64::from_le_bytes(data[92..100].try_into().unwrap());
        assert_eq!(count, 2);
        // Verify hash of EFI_GPT_DATA structure
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(&data);
        assert_eq!(
            hex::encode(hash),
            "4b92d9fa24b7aabecdeb488a164a69ad7d152d47a69d080b8cab83009798f6dd"
        );
    }

    #[test]
    fn test_set_gpt_priority_bits() {
        let mut gpt = mock_gpt();
        set_gpt_priority_bits(&mut gpt, 1, 2, 3, true).unwrap();

        let attr_offset = 512 + 48;
        let attrs = u64::from_le_bytes(gpt[attr_offset..attr_offset + 8].try_into().unwrap());
        let prio = GptPrio::from(attrs);

        assert_eq!(prio.priority().into_inner(), 2);
        assert_eq!(prio.tries_left().into_inner(), 3);
        assert!(prio.successful());
    }

    #[test]
    fn test_set_private_succeeded() {
        let mut gpt = mock_gpt();
        set_private_succeeded(&mut gpt, 2, true);

        let attr_offset = 512 + 128 + 48; // Partition 2
        let attrs = u64::from_le_bytes(gpt[attr_offset..attr_offset + 8].try_into().unwrap());
        let prio = GptPrio::from(attrs);

        assert!(prio.has_boot_succeeded());
    }

    #[test]
    fn test_recalculate_gpt_crcs() {
        let mut gpt = mock_gpt();
        recalculate_gpt_crcs(&mut gpt);

        let header_crc = u32::from_le_bytes(gpt[16..20].try_into().unwrap());
        let partition_crc = u32::from_le_bytes(gpt[88..92].try_into().unwrap());
        assert_eq!(header_crc, 0x1f142870);
        assert_eq!(partition_crc, 0x8c013c00);
    }

    #[test]
    fn test_extract_primary_gpt() {
        // Create a mock disk: MBR (512) + GPT header (512) + entries (32*512)
        let mut disk = vec![0u8; 512 + 33 * 512];

        // Put GPT signature at LBA 1
        disk[512..520].copy_from_slice(b"EFI PART");

        let mut cursor = Cursor::new(&mut disk[..]);
        let gpt = extract_primary_gpt(&mut cursor).unwrap();

        assert_eq!(gpt.len(), 33 * 512);
        assert_eq!(&gpt[0..8], b"EFI PART");
    }

    /// Create a mock disk with valid GPT for find_partitions testing.
    fn mock_disk_with_partitions() -> Vec<u8> {
        // Disk: MBR + GPT header + partition entries + backup
        let mut disk = vec![0u8; 512 * 68]; // MBR + header + 33 entry sectors + backup header

        // Protective MBR at LBA 0
        disk[446] = 0x00; // Boot indicator
        disk[450] = 0xEE; // Partition type (GPT protective)
        disk[510] = 0x55; // MBR signature
        disk[511] = 0xAA;

        // GPT header at LBA 1 (offset 512)
        let hdr = 512;
        disk[hdr..hdr + 8].copy_from_slice(b"EFI PART");
        disk[hdr + 8..hdr + 12].copy_from_slice(&0x00010000u32.to_le_bytes()); // Revision
        disk[hdr + 12..hdr + 16].copy_from_slice(&92u32.to_le_bytes()); // Header size

        // CRC32 at offset 16 - will set after

        disk[hdr + 20..hdr + 24].copy_from_slice(&0u32.to_le_bytes()); // Reserved
        disk[hdr + 24..hdr + 32].copy_from_slice(&1u64.to_le_bytes()); // Current LBA
        disk[hdr + 32..hdr + 40].copy_from_slice(&67u64.to_le_bytes()); // Backup LBA
        disk[hdr + 40..hdr + 48].copy_from_slice(&34u64.to_le_bytes()); // First usable LBA
        disk[hdr + 48..hdr + 56].copy_from_slice(&66u64.to_le_bytes()); // Last usable LBA

        disk[hdr + 56..hdr + 72].copy_from_slice(&[1u8; 16]); // Disk GUID at offset 56 (16 bytes)
        disk[hdr + 72..hdr + 80].copy_from_slice(&2u64.to_le_bytes()); // Partition entry LBA
        disk[hdr + 80..hdr + 84].copy_from_slice(&128u32.to_le_bytes()); // Number of entries
        disk[hdr + 84..hdr + 88].copy_from_slice(&128u32.to_le_bytes()); // Entry size

        // Partition array CRC at offset 88 - will set after

        // Partition entries at LBA 2 (offset 1024)
        let entries = 1024;

        // Partition 1: EFI System Partition
        disk[entries..entries + 16].copy_from_slice(&EFI_SYSTEM_PARTITION);
        disk[entries + 16..entries + 32].copy_from_slice(&[0x11; 16]); // Unique GUID
        disk[entries + 32..entries + 40].copy_from_slice(&34u64.to_le_bytes()); // Start LBA
        disk[entries + 40..entries + 48].copy_from_slice(&40u64.to_le_bytes()); // End LBA

        // Partition 2: BOOT-A
        let p2 = entries + 128;
        disk[p2..p2 + 16].copy_from_slice(&BOTTLEROCKET_BOOT);
        disk[p2 + 16..p2 + 32].copy_from_slice(&[0x22; 16]);
        disk[p2 + 32..p2 + 40].copy_from_slice(&41u64.to_le_bytes());
        disk[p2 + 40..p2 + 48].copy_from_slice(&50u64.to_le_bytes());

        // Partition 3: BOOT-B
        let p3 = entries + 256;
        disk[p3..p3 + 16].copy_from_slice(&BOTTLEROCKET_BOOT);
        disk[p3 + 16..p3 + 32].copy_from_slice(&[0x33; 16]);
        disk[p3 + 32..p3 + 40].copy_from_slice(&51u64.to_le_bytes());
        disk[p3 + 40..p3 + 48].copy_from_slice(&60u64.to_le_bytes());

        // Partition 4: PRIVATE
        let p4 = entries + 384;
        disk[p4..p4 + 16].copy_from_slice(&BOTTLEROCKET_PRIVATE);
        disk[p4 + 16..p4 + 32].copy_from_slice(&[0x44; 16]);
        disk[p4 + 32..p4 + 40].copy_from_slice(&61u64.to_le_bytes());
        disk[p4 + 40..p4 + 48].copy_from_slice(&66u64.to_le_bytes());

        // Calculate partition array CRC
        let part_crc = crc32fast::hash(&disk[entries..entries + 128 * 128]);
        disk[hdr + 88..hdr + 92].copy_from_slice(&part_crc.to_le_bytes());

        // Calculate header CRC (with CRC field zeroed)
        let header_crc = crc32fast::hash(&disk[hdr..hdr + 92]);
        disk[hdr + 16..hdr + 20].copy_from_slice(&header_crc.to_le_bytes());

        disk
    }

    #[test]
    fn test_find_partitions() {
        let disk = mock_disk_with_partitions();
        let mut cursor = Cursor::new(&disk[..]);

        let layout = find_partitions(&mut cursor).unwrap();

        assert_eq!(layout.efi_a.number, 1);
        assert_eq!(layout.efi_a.start_lba, 34);
        assert_eq!(layout.boot_a.number, 2);
        assert_eq!(layout.boot_b.as_ref().unwrap().number, 3);
        assert_eq!(layout.private.number, 4);
    }

    /// Recalculate GPT CRCs for a full disk image (GPT at offset 512).
    fn recalc_disk_crcs(disk: &mut [u8]) {
        let part_crc = crc32fast::hash(&disk[1024..1024 + 128 * 128]);
        disk[512 + 88..512 + 92].copy_from_slice(&part_crc.to_le_bytes());
        disk[512 + 16..512 + 20].copy_from_slice(&[0u8; 4]);
        let header_crc = crc32fast::hash(&disk[512..512 + 92]);
        disk[512 + 16..512 + 20].copy_from_slice(&header_crc.to_le_bytes());
    }

    #[test]
    fn test_find_partitions_missing_efi() {
        let mut disk = mock_disk_with_partitions();
        disk[1024..1024 + 16].copy_from_slice(&[0u8; 16]);
        recalc_disk_crcs(&mut disk);

        let mut cursor = Cursor::new(&disk[..]);
        let err = find_partitions(&mut cursor).unwrap_err();
        assert!(err.to_string().contains("EFI-A"));
    }

    #[test]
    fn test_find_partitions_missing_boot() {
        let mut disk = mock_disk_with_partitions();
        disk[1024 + 128..1024 + 128 + 16].copy_from_slice(&[0u8; 16]);
        disk[1024 + 256..1024 + 256 + 16].copy_from_slice(&[0u8; 16]);
        recalc_disk_crcs(&mut disk);

        let mut cursor = Cursor::new(&disk[..]);
        let err = find_partitions(&mut cursor).unwrap_err();
        assert!(err.to_string().contains("BOOT-A"));
    }

    #[test]
    fn test_find_partitions_missing_private() {
        let mut disk = mock_disk_with_partitions();
        disk[1024 + 384..1024 + 384 + 16].copy_from_slice(&[0u8; 16]);
        recalc_disk_crcs(&mut disk);

        let mut cursor = Cursor::new(&disk[..]);
        let err = find_partitions(&mut cursor).unwrap_err();
        assert!(err.to_string().contains("PRIVATE"));
    }

    #[test]
    fn test_find_partitions_invalid_gpt() {
        let disk = vec![0u8; 512 * 68];
        let mut cursor = Cursor::new(&disk[..]);
        let err = find_partitions(&mut cursor).unwrap_err();
        assert!(err.to_string().contains("GPT"));
    }
}
