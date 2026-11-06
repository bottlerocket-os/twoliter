//! Filesystem extraction for boot components.
//!
//! Extracts shim, grub, vmlinuz, grub.cfg, and bootconfig from disk image partitions.

use crate::error::Result;

use ext4_view::Ext4;
use fatfs::{FileSystem, FsOptions};
use snafu::{ensure_whatever, whatever, OptionExt, ResultExt};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::ops::RangeInclusive;

/// A mounted EFI System Partition.
pub type Esp = FileSystem<Cursor<Vec<u8>>>;

fn read_partition<R: Read + Seek>(
    disk: &mut R,
    bytes: RangeInclusive<u64>,
    name: &str,
) -> Result<Vec<u8>> {
    disk.seek(SeekFrom::Start(*bytes.start()))
        .with_whatever_context(|_| format!("failed to seek to {name}"))?;
    let size = usize::try_from(bytes.end() - bytes.start() + 1)
        .with_whatever_context(|_| format!("{name} size overflow"))?;
    let mut data = vec![0; size];
    disk.read_exact(&mut data)
        .with_whatever_context(|_| format!("failed to read {name}"))?;
    Ok(data)
}

/// Copy the ESP into memory and mount it.
pub fn mount_esp<R: Read + Seek>(disk: &mut R, bytes: RangeInclusive<u64>) -> Result<Esp> {
    let data = read_partition(disk, bytes, "ESP")?;
    FileSystem::new(Cursor::new(data), FsOptions::new()).whatever_context("failed to mount ESP")
}

/// Copy an ext4 partition into memory and mount it.
pub fn mount_ext4<R: Read + Seek>(
    disk: &mut R,
    bytes: RangeInclusive<u64>,
    name: &str,
) -> Result<Ext4> {
    let data = read_partition(disk, bytes, name)?;
    Ext4::load(Box::new(data)).with_whatever_context(|_| format!("failed to mount {name}"))
}

/// Read the single architecture-specific firmware fallback executable.
pub fn read_fallback(esp: &Esp) -> Result<Vec<u8>> {
    let root = esp.root_dir();
    let mut found = None;
    // The UEFI removable-media path: shim on GRUB images, the UKI itself on UKI images.
    for path in ["EFI/BOOT/bootx64.efi", "EFI/BOOT/bootaa64.efi"] {
        match root.open_file(path) {
            Ok(mut file) => {
                ensure_whatever!(
                    found.is_none(),
                    "ambiguous ESP: multiple fallback executables"
                );
                let mut data = Vec::new();
                file.read_to_end(&mut data)
                    .whatever_context("failed to read fallback executable")?;
                found = Some(data);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).whatever_context("failed to open fallback executable"),
        }
    }
    found.whatever_context("no firmware fallback executable on ESP")
}

/// Read `grubaa64.efi` or `grubx64.efi` from `/EFI/BOOT/`.
pub fn read_grub(esp: &Esp) -> Result<Vec<u8>> {
    let names = ["grubaa64.efi", "grubx64.efi"];
    let boot_dir = esp
        .root_dir()
        .open_dir("EFI/BOOT")
        .whatever_context("EFI/BOOT directory not found")?;

    for name in names {
        if let Ok(mut file) = boot_dir.open_file(name) {
            let mut contents = Vec::new();
            file.read_to_end(&mut contents)
                .whatever_context("failed to read EFI file")?;
            return Ok(contents);
        }
    }

    whatever!("none of {:?} found in /EFI/BOOT/", names);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a minimal ext2 filesystem image with /vmlinuz, /grub/grub.cfg, and /bootconfig.data.
    fn generate_boot_image() -> Vec<u8> {
        const SIZE: usize = 1024 * 1024; // 1MB
        const BLOCK_SIZE: usize = 1024;
        const INODE_SIZE: usize = 256; // ext4-view expects 256
        const INODES_PER_GROUP: u32 = 128;
        const BLOCKS_COUNT: u32 = (SIZE / BLOCK_SIZE) as u32;

        let mut img = vec![0u8; SIZE];

        // Superblock at offset 1024 (block 1)
        let sb_offset = 1024;
        let sb = &mut img[sb_offset..sb_offset + 1024];

        // s_inodes_count
        sb[0..4].copy_from_slice(&INODES_PER_GROUP.to_le_bytes());
        // s_blocks_count
        sb[4..8].copy_from_slice(&BLOCKS_COUNT.to_le_bytes());
        // s_r_blocks_count (reserved)
        sb[8..12].copy_from_slice(&51u32.to_le_bytes());
        // s_free_blocks_count
        sb[12..16].copy_from_slice(&(BLOCKS_COUNT - 46).to_le_bytes());
        // s_free_inodes_count
        sb[16..20].copy_from_slice(&(INODES_PER_GROUP - 15).to_le_bytes());
        // s_first_data_block
        sb[20..24].copy_from_slice(&1u32.to_le_bytes());
        // s_log_block_size (0 = 1024 bytes)
        sb[24..28].copy_from_slice(&0u32.to_le_bytes());
        // s_log_cluster_size
        sb[28..32].copy_from_slice(&0u32.to_le_bytes());
        // s_blocks_per_group
        sb[32..36].copy_from_slice(&8192u32.to_le_bytes());
        // s_clusters_per_group
        sb[36..40].copy_from_slice(&8192u32.to_le_bytes());
        // s_inodes_per_group
        sb[40..44].copy_from_slice(&INODES_PER_GROUP.to_le_bytes());
        // s_magic
        sb[56..58].copy_from_slice(&0xEF53u16.to_le_bytes());
        // s_state (clean)
        sb[58..60].copy_from_slice(&1u16.to_le_bytes());
        // s_errors
        sb[60..62].copy_from_slice(&1u16.to_le_bytes());
        // s_rev_level (dynamic)
        sb[76..80].copy_from_slice(&1u32.to_le_bytes());
        // s_first_ino
        sb[84..88].copy_from_slice(&11u32.to_le_bytes());
        // s_inode_size
        sb[88..90].copy_from_slice(&(INODE_SIZE as u16).to_le_bytes());
        // s_feature_incompat = FILETYPE
        sb[96..100].copy_from_slice(&0x0002u32.to_le_bytes());

        // Group descriptor at block 2 (offset 2048)
        let gd_offset = 2 * BLOCK_SIZE;
        let gd = &mut img[gd_offset..gd_offset + 32];
        // bg_block_bitmap
        gd[0..4].copy_from_slice(&3u32.to_le_bytes());
        // bg_inode_bitmap
        gd[4..8].copy_from_slice(&4u32.to_le_bytes());
        // bg_inode_table (starts at block 5, needs 32 blocks for 128 inodes * 256 bytes)
        gd[8..12].copy_from_slice(&5u32.to_le_bytes());
        // bg_free_blocks_count
        gd[12..14].copy_from_slice(&(BLOCKS_COUNT as u16 - 46).to_le_bytes());
        // bg_free_inodes_count
        gd[14..16].copy_from_slice(&(INODES_PER_GROUP as u16 - 15).to_le_bytes());
        // bg_used_dirs_count
        gd[16..18].copy_from_slice(&2u16.to_le_bytes());

        // Block bitmap at block 3 - mark first 46 blocks as used
        let bb_offset = 3 * BLOCK_SIZE;
        img[bb_offset..bb_offset + 5].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        img[bb_offset + 5] = 0x3F; // blocks 40-45 used

        // Inode bitmap at block 4 - mark first 15 inodes as used
        let ib_offset = 4 * BLOCK_SIZE;
        img[ib_offset] = 0xFF;
        img[ib_offset + 1] = 0x7F; // inodes 1-15 used

        // Inode table starts at block 5 (256 bytes per inode)
        let it_offset = 5 * BLOCK_SIZE;

        // Root inode (inode 2) at offset 256 (inode 1 is bad blocks)
        let root_inode_offset = it_offset + INODE_SIZE;
        let root = &mut img[root_inode_offset..root_inode_offset + INODE_SIZE];
        // i_mode = S_IFDIR | 0755
        root[0..2].copy_from_slice(&0x41EDu16.to_le_bytes());
        // i_size = 1024 (one block)
        root[4..8].copy_from_slice(&1024u32.to_le_bytes());
        // i_links_count
        root[26..28].copy_from_slice(&3u16.to_le_bytes()); // . + .. + grub subdir

        // i_blocks (in 512-byte units)
        root[28..32].copy_from_slice(&2u32.to_le_bytes());
        // i_block[0] = block 40 (root dir data)
        root[40..44].copy_from_slice(&40u32.to_le_bytes());

        // vmlinuz inode (inode 12)
        let vmlinuz_inode_offset = it_offset + 11 * INODE_SIZE;
        let vmlinuz_inode = &mut img[vmlinuz_inode_offset..vmlinuz_inode_offset + INODE_SIZE];
        // i_mode = S_IFREG | 0644
        vmlinuz_inode[0..2].copy_from_slice(&0x81A4u16.to_le_bytes());
        // i_size = 7 bytes
        vmlinuz_inode[4..8].copy_from_slice(&7u32.to_le_bytes());
        // i_links_count
        vmlinuz_inode[26..28].copy_from_slice(&1u16.to_le_bytes());
        // i_blocks
        vmlinuz_inode[28..32].copy_from_slice(&2u32.to_le_bytes());
        // i_block[0] = block 41 (vmlinuz data)
        vmlinuz_inode[40..44].copy_from_slice(&41u32.to_le_bytes());

        // grub directory inode (inode 13)
        let grub_dir_inode_offset = it_offset + 12 * INODE_SIZE;
        let grub_dir = &mut img[grub_dir_inode_offset..grub_dir_inode_offset + INODE_SIZE];
        grub_dir[0..2].copy_from_slice(&0x41EDu16.to_le_bytes()); // S_IFDIR | 0755
        grub_dir[4..8].copy_from_slice(&1024u32.to_le_bytes());
        grub_dir[26..28].copy_from_slice(&2u16.to_le_bytes());
        grub_dir[28..32].copy_from_slice(&2u32.to_le_bytes());
        grub_dir[40..44].copy_from_slice(&42u32.to_le_bytes()); // block 42

        // grub.cfg inode (inode 14)
        let grub_cfg_inode_offset = it_offset + 13 * INODE_SIZE;
        let grub_cfg = &mut img[grub_cfg_inode_offset..grub_cfg_inode_offset + INODE_SIZE];
        grub_cfg[0..2].copy_from_slice(&0x81A4u16.to_le_bytes()); // S_IFREG | 0644
        grub_cfg[4..8].copy_from_slice(&8u32.to_le_bytes()); // 8 bytes
        grub_cfg[26..28].copy_from_slice(&1u16.to_le_bytes());
        grub_cfg[28..32].copy_from_slice(&2u32.to_le_bytes());
        grub_cfg[40..44].copy_from_slice(&43u32.to_le_bytes()); // block 43

        // bootconfig.data inode (inode 15)
        let bc_inode_offset = it_offset + 14 * INODE_SIZE;
        let bc = &mut img[bc_inode_offset..bc_inode_offset + INODE_SIZE];
        bc[0..2].copy_from_slice(&0x81A4u16.to_le_bytes()); // S_IFREG | 0644
        bc[4..8].copy_from_slice(&15u32.to_le_bytes()); // 15 bytes
        bc[26..28].copy_from_slice(&1u16.to_le_bytes());
        bc[28..32].copy_from_slice(&2u32.to_le_bytes());
        bc[40..44].copy_from_slice(&44u32.to_le_bytes()); // block 44

        // Root directory data at block 40
        let root_dir_offset = 40 * BLOCK_SIZE;
        let root_dir = &mut img[root_dir_offset..root_dir_offset + BLOCK_SIZE];

        // "." entry
        root_dir[0..4].copy_from_slice(&2u32.to_le_bytes()); // inode
        root_dir[4..6].copy_from_slice(&12u16.to_le_bytes()); // rec_len
        root_dir[6] = 1; // name_len
        root_dir[7] = 2; // file_type = EXT2_FT_DIR
        root_dir[8] = b'.';

        // ".." entry
        root_dir[12..16].copy_from_slice(&2u32.to_le_bytes()); // inode
        root_dir[16..18].copy_from_slice(&12u16.to_le_bytes()); // rec_len
        root_dir[18] = 2; // name_len
        root_dir[19] = 2; // file_type = EXT2_FT_DIR
        root_dir[20..22].copy_from_slice(b"..");

        // "vmlinuz" entry
        root_dir[24..28].copy_from_slice(&12u32.to_le_bytes()); // inode 12
        root_dir[28..30].copy_from_slice(&16u16.to_le_bytes()); // rec_len
        root_dir[30] = 7; // name_len
        root_dir[31] = 1; // file_type = EXT2_FT_REG_FILE
        root_dir[32..39].copy_from_slice(b"vmlinuz");

        // "grub" directory entry
        root_dir[40..44].copy_from_slice(&13u32.to_le_bytes()); // inode 13
        root_dir[44..46].copy_from_slice(&16u16.to_le_bytes()); // rec_len
        root_dir[46] = 4; // name_len
        root_dir[47] = 2; // file_type = EXT2_FT_DIR
        root_dir[48..52].copy_from_slice(b"grub");

        // "bootconfig.data" entry
        root_dir[56..60].copy_from_slice(&15u32.to_le_bytes()); // inode 15
        root_dir[60..62].copy_from_slice(&968u16.to_le_bytes()); // rec_len (rest of block)
        root_dir[62] = 15; // name_len
        root_dir[63] = 1; // file_type = EXT2_FT_REG_FILE
        root_dir[64..79].copy_from_slice(b"bootconfig.data");

        // vmlinuz file data at block 41
        let vmlinuz_data_offset = 41 * BLOCK_SIZE;
        img[vmlinuz_data_offset..vmlinuz_data_offset + 7].copy_from_slice(b"vmlinuz");

        // grub directory data at block 42
        let grub_dir_offset = 42 * BLOCK_SIZE;
        let grub_dir_data = &mut img[grub_dir_offset..grub_dir_offset + BLOCK_SIZE];
        // "."
        grub_dir_data[0..4].copy_from_slice(&13u32.to_le_bytes());
        grub_dir_data[4..6].copy_from_slice(&12u16.to_le_bytes());
        grub_dir_data[6] = 1;
        grub_dir_data[7] = 2;
        grub_dir_data[8] = b'.';
        // ".."
        grub_dir_data[12..16].copy_from_slice(&2u32.to_le_bytes());
        grub_dir_data[16..18].copy_from_slice(&12u16.to_le_bytes());
        grub_dir_data[18] = 2;
        grub_dir_data[19] = 2;
        grub_dir_data[20..22].copy_from_slice(b"..");
        // "grub.cfg"
        grub_dir_data[24..28].copy_from_slice(&14u32.to_le_bytes()); // inode 14
        grub_dir_data[28..30].copy_from_slice(&1000u16.to_le_bytes());
        grub_dir_data[30] = 8; // name_len
        grub_dir_data[31] = 1; // file_type = EXT2_FT_REG_FILE
        grub_dir_data[32..40].copy_from_slice(b"grub.cfg");

        // grub.cfg file data at block 43
        let grub_cfg_offset = 43 * BLOCK_SIZE;
        img[grub_cfg_offset..grub_cfg_offset + 8].copy_from_slice(b"grub.cfg");

        // bootconfig.data file data at block 44
        let bc_offset = 44 * BLOCK_SIZE;
        img[bc_offset..bc_offset + 15].copy_from_slice(b"bootconfig.data");

        img
    }

    /// Generate a minimal FAT12 filesystem image with /EFI/BOOT/ directory structure.
    fn generate_esp_image() -> Vec<u8> {
        const SIZE: usize = 1024 * 1024; // 1MB
        const SECTOR_SIZE: usize = 512;
        const CLUSTER_SIZE: usize = SECTOR_SIZE * 4; // 4 sectors per cluster
        const RESERVED_SECTORS: usize = 1;
        const NUM_FATS: usize = 2;
        const ROOT_ENTRIES: usize = 512;
        const ROOT_DIR_SECTORS: usize = (ROOT_ENTRIES * 32) / SECTOR_SIZE;
        const TOTAL_SECTORS: usize = SIZE / SECTOR_SIZE;
        const FAT_SECTORS: usize = 2; // enough for FAT12 on 1MB

        let mut img = vec![0u8; SIZE];

        // Boot sector (BPB)
        img[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]); // Jump + NOP
        img[3..11].copy_from_slice(b"mkfs.fat"); // OEM name
        img[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes()); // bytes/sector
        img[13] = (CLUSTER_SIZE / SECTOR_SIZE) as u8; // sectors/cluster
        img[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes()); // reserved sectors
        img[16] = NUM_FATS as u8; // number of FATs
        img[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes()); // root entries
        img[19..21].copy_from_slice(&(TOTAL_SECTORS as u16).to_le_bytes()); // total sectors
        img[21] = 0xF8; // media type (fixed disk)
        img[22..24].copy_from_slice(&(FAT_SECTORS as u16).to_le_bytes()); // sectors/FAT
        img[24..26].copy_from_slice(&16u16.to_le_bytes()); // sectors/track
        img[26..28].copy_from_slice(&2u16.to_le_bytes()); // heads
        img[38] = 0x29; // extended boot signature
        img[39..43].copy_from_slice(&0x12345678u32.to_le_bytes()); // volume ID
        img[43..54].copy_from_slice(b"NO NAME    "); // volume label
        img[54..62].copy_from_slice(b"FAT12   "); // filesystem type
        img[510..512].copy_from_slice(&[0x55, 0xAA]); // boot signature

        let fat_start = RESERVED_SECTORS * SECTOR_SIZE;
        let root_dir_start = fat_start + NUM_FATS * FAT_SECTORS * SECTOR_SIZE;
        let data_start = root_dir_start + ROOT_DIR_SECTORS * SECTOR_SIZE;

        // Helper to write FAT12 entry
        let write_fat12 = |fat: &mut [u8], cluster: usize, value: u16| {
            let offset = cluster + cluster / 2;
            if cluster.is_multiple_of(2) {
                fat[offset] = (value & 0xFF) as u8;
                fat[offset + 1] = (fat[offset + 1] & 0xF0) | ((value >> 8) & 0x0F) as u8;
            } else {
                fat[offset] = (fat[offset] & 0x0F) | ((value << 4) & 0xF0) as u8;
                fat[offset + 1] = (value >> 4) as u8;
            }
        };

        // Initialize FAT (both copies)
        for fat_num in 0..NUM_FATS {
            let fat = &mut img[fat_start + fat_num * FAT_SECTORS * SECTOR_SIZE..];
            fat[0] = 0xF8; // media byte
            fat[1] = 0xFF;
            fat[2] = 0xFF;
            // Cluster 2: EFI dir -> EOF
            write_fat12(fat, 2, 0xFFF);
            // Cluster 3: BOOT dir -> EOF
            write_fat12(fat, 3, 0xFFF);
            // Cluster 4: bootaa64.efi -> EOF
            write_fat12(fat, 4, 0xFFF);
            // Cluster 5: grubaa64.efi -> EOF
            write_fat12(fat, 5, 0xFFF);
        }

        // Helper to create directory entry
        let make_dir_entry = |name: &[u8; 11], attr: u8, cluster: u16, size: u32| -> [u8; 32] {
            let mut entry = [0u8; 32];
            entry[0..11].copy_from_slice(name);
            entry[11] = attr;
            entry[26..28].copy_from_slice(&cluster.to_le_bytes());
            entry[28..32].copy_from_slice(&size.to_le_bytes());
            entry
        };

        // Root directory: EFI entry
        let efi_entry = make_dir_entry(b"EFI        ", 0x10, 2, 0);
        img[root_dir_start..root_dir_start + 32].copy_from_slice(&efi_entry);

        // EFI directory (cluster 2): . and .. and BOOT
        let cluster2_offset = data_start;
        let dot = make_dir_entry(b".          ", 0x10, 2, 0);
        let dotdot = make_dir_entry(b"..         ", 0x10, 0, 0);
        let boot_entry = make_dir_entry(b"BOOT       ", 0x10, 3, 0);
        img[cluster2_offset..cluster2_offset + 32].copy_from_slice(&dot);
        img[cluster2_offset + 32..cluster2_offset + 64].copy_from_slice(&dotdot);
        img[cluster2_offset + 64..cluster2_offset + 96].copy_from_slice(&boot_entry);

        // BOOT directory (cluster 3): . and .. and files
        let cluster3_offset = data_start + CLUSTER_SIZE;
        let dot = make_dir_entry(b".          ", 0x10, 3, 0);
        let dotdot = make_dir_entry(b"..         ", 0x10, 2, 0);
        let bootaa64 = make_dir_entry(b"BOOTAA64EFI", 0x00, 4, 12);
        let grubaa64 = make_dir_entry(b"GRUBAA64EFI", 0x00, 5, 12);
        img[cluster3_offset..cluster3_offset + 32].copy_from_slice(&dot);
        img[cluster3_offset + 32..cluster3_offset + 64].copy_from_slice(&dotdot);
        img[cluster3_offset + 64..cluster3_offset + 96].copy_from_slice(&bootaa64);
        img[cluster3_offset + 96..cluster3_offset + 128].copy_from_slice(&grubaa64);

        // File data
        let cluster4_offset = data_start + CLUSTER_SIZE * 2;
        let cluster5_offset = data_start + CLUSTER_SIZE * 3;
        img[cluster4_offset..cluster4_offset + 12].copy_from_slice(b"bootaa64.efi");
        img[cluster5_offset..cluster5_offset + 12].copy_from_slice(b"grubaa64.efi");

        img
    }

    /// The 1MB test filesystem images start at offset 0.
    fn whole_image() -> RangeInclusive<u64> {
        0..=1024 * 1024 - 1
    }

    #[test]
    fn test_extract_shim() {
        let esp_img = generate_esp_image();
        let mut cursor = std::io::Cursor::new(&esp_img);
        let shim = mount_esp(&mut cursor, whole_image())
            .and_then(|esp| read_fallback(&esp))
            .unwrap();
        assert_eq!(shim, b"bootaa64.efi");
    }

    #[test]
    fn test_extract_grub() {
        let esp_img = generate_esp_image();
        let mut cursor = std::io::Cursor::new(&esp_img);
        let grub = read_grub(&mount_esp(&mut cursor, whole_image()).unwrap()).unwrap();
        assert_eq!(grub, b"grubaa64.efi");
    }

    #[test]
    fn test_extract_vmlinuz() {
        let boot_img = generate_boot_image();
        let mut cursor = std::io::Cursor::new(&boot_img);
        let vmlinuz = mount_ext4(&mut cursor, whole_image(), "BOOT")
            .unwrap()
            .read("/vmlinuz")
            .unwrap();
        assert_eq!(vmlinuz, b"vmlinuz");
    }

    /// Generate ESP image with only x86_64 files (no aarch64).
    fn generate_esp_image_x64() -> Vec<u8> {
        let mut img = generate_esp_image();

        // Constants from generate_esp_image
        const SECTOR_SIZE: usize = 512;
        const CLUSTER_SIZE: usize = SECTOR_SIZE * 4;
        const RESERVED_SECTORS: usize = 1;
        const NUM_FATS: usize = 2;
        const FAT_SECTORS: usize = 2;
        const ROOT_ENTRIES: usize = 512;
        const ROOT_DIR_SECTORS: usize = (ROOT_ENTRIES * 32) / SECTOR_SIZE;

        let fat_start = RESERVED_SECTORS * SECTOR_SIZE;
        let root_dir_start = fat_start + NUM_FATS * FAT_SECTORS * SECTOR_SIZE;
        let data_start = root_dir_start + ROOT_DIR_SECTORS * SECTOR_SIZE;
        let cluster3_offset = data_start + CLUSTER_SIZE;

        // Replace BOOTAA64EFI with BOOTX64 EFI in BOOT directory (8.3 format)
        img[cluster3_offset + 64..cluster3_offset + 75].copy_from_slice(b"BOOTX64 EFI");
        img[cluster3_offset + 96..cluster3_offset + 107].copy_from_slice(b"GRUBX64 EFI");

        // Update file data (content doesn't matter for this test, just needs to be readable)
        let cluster4_offset = data_start + CLUSTER_SIZE * 2;
        let cluster5_offset = data_start + CLUSTER_SIZE * 3;
        img[cluster4_offset..cluster4_offset + 12].copy_from_slice(b"bootx64.efi\0");
        img[cluster5_offset..cluster5_offset + 12].copy_from_slice(b"grubx64.efi\0");

        img
    }

    #[test]
    fn test_extract_shim_x64_fallback() {
        let esp_img = generate_esp_image_x64();
        let mut cursor = std::io::Cursor::new(&esp_img);
        let shim = mount_esp(&mut cursor, whole_image())
            .and_then(|esp| read_fallback(&esp))
            .unwrap();
        assert!(shim.starts_with(b"bootx64.efi"));
    }

    #[test]
    fn test_extract_grub_x64_fallback() {
        let esp_img = generate_esp_image_x64();
        let mut cursor = std::io::Cursor::new(&esp_img);
        let grub = read_grub(&mount_esp(&mut cursor, whole_image()).unwrap()).unwrap();
        assert!(grub.starts_with(b"grubx64.efi"));
    }

    #[test]
    fn test_extract_shim_not_found() {
        // ESP with no boot files - just EFI/BOOT directory
        let mut img = generate_esp_image();

        const SECTOR_SIZE: usize = 512;
        const CLUSTER_SIZE: usize = SECTOR_SIZE * 4;
        const RESERVED_SECTORS: usize = 1;
        const NUM_FATS: usize = 2;
        const FAT_SECTORS: usize = 2;
        const ROOT_ENTRIES: usize = 512;
        const ROOT_DIR_SECTORS: usize = (ROOT_ENTRIES * 32) / SECTOR_SIZE;

        let fat_start = RESERVED_SECTORS * SECTOR_SIZE;
        let root_dir_start = fat_start + NUM_FATS * FAT_SECTORS * SECTOR_SIZE;
        let data_start = root_dir_start + ROOT_DIR_SECTORS * SECTOR_SIZE;
        let cluster3_offset = data_start + CLUSTER_SIZE;

        // Zero out file entries in BOOT directory (keep . and ..)
        img[cluster3_offset + 64..cluster3_offset + 128].fill(0);

        let mut cursor = std::io::Cursor::new(&img);
        let err = mount_esp(&mut cursor, whole_image())
            .and_then(|esp| read_fallback(&esp))
            .unwrap_err();
        assert!(err.to_string().contains("fallback"));
    }
}
