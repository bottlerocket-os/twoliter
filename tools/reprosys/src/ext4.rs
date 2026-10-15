//! The metadata of ext4 filesystem entries that `ext4_view` does not expose:
//! extended attributes and device numbers. Directories are walked from the
//! raw inodes and their extent trees; every entry is cross-checked against
//! `ext4_view`, so that a misread fails the comparison rather than hiding a
//! difference. Inline-data directories and xattr values stored in inodes
//! (`ea_inode`) are not supported.

use crate::error::Result;
use crate::fsfiles::{self, display, Entry, FsPath, Kind, Xattrs, S_IFBLK, S_IFCHR, S_IFMT};
use ext4_view::{Ext4, FileType};
use snafu::{ensure_whatever, OptionExt, ResultExt};
use std::collections::{BTreeMap, BTreeSet};

/// Offset of the primary superblock.
const SUPERBLOCK: usize = 1024;
const MAGIC: u16 = 0xef53;
/// INCOMPAT_RECOVER: the journal holds transactions the next mount replays.
const INCOMPAT_RECOVER: u32 = 0x4;
const INCOMPAT_64BIT: u32 = 0x80;
const ROOT_INODE: u32 = 2;
/// The inode flags of inodes whose blocks an extent tree maps, and of inline data.
const EXTENTS_FL: u32 = 0x8_0000;
const INLINE_DATA_FL: u32 = 0x1000_0000;
const EXTENT_MAGIC: u16 = 0xf30a;
/// The depth limit of extent trees, as in the kernel.
const MAX_EXTENT_DEPTH: u16 = 5;
const XATTR_MAGIC: u32 = 0xea02_0000;
/// The size of an EA block header, after which its entries start.
const XATTR_BLOCK_HEADER: usize = 32;
/// The size of an xattr entry before its name.
const XATTR_ENTRY: usize = 16;
/// The size of the inode fields every inode has, before `i_extra_isize`.
const GOOD_OLD_INODE_SIZE: usize = 128;

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().expect("4 bytes"))
}

/// Whether the published filesystem has a journal to replay when mounted,
/// which would change it beyond what is compared.
pub fn needs_recovery(partition: &[u8]) -> Result<bool> {
    let fs = Filesystem::new(partition)?;
    Ok(fs.incompat & INCOMPAT_RECOVER != 0)
}

/// The fields of an ext4 filesystem needed to find inodes and their blocks.
struct Filesystem<'a> {
    data: &'a [u8],
    block_size: usize,
    inode_size: usize,
    inodes_per_group: u32,
    inodes_count: u32,
    first_data_block: u64,
    desc_size: usize,
    incompat: u32,
}

/// The attributes read from an inode.
struct Inode<'a> {
    raw: &'a [u8],
    mode: u16,
    flags: u32,
    size: u64,
}

impl<'a> Filesystem<'a> {
    fn new(data: &'a [u8]) -> Result<Self> {
        let sb = data
            .get(SUPERBLOCK..SUPERBLOCK + 1024)
            .whatever_context("ext4 filesystem is truncated")?;
        ensure_whatever!(u16_at(sb, 0x38) == MAGIC, "no ext4 superblock");
        let log_block_size = u32_at(sb, 0x18);
        ensure_whatever!(log_block_size <= 6, "invalid ext4 block size");
        let incompat = u32_at(sb, 0x60);
        // Revision 0 filesystems have 128-byte inodes and no `s_inode_size`.
        let inode_size = if u32_at(sb, 0x4c) == 0 {
            GOOD_OLD_INODE_SIZE
        } else {
            usize::from(u16_at(sb, 0x58))
        };
        ensure_whatever!(
            inode_size >= GOOD_OLD_INODE_SIZE && inode_size.is_power_of_two(),
            "invalid ext4 inode size {inode_size}"
        );
        let desc_size = if incompat & INCOMPAT_64BIT != 0 {
            usize::from(u16_at(sb, 0xfe))
        } else {
            32
        };
        ensure_whatever!(desc_size >= 32, "invalid ext4 group descriptor size");
        let inodes_per_group = u32_at(sb, 0x28);
        ensure_whatever!(inodes_per_group > 0, "invalid ext4 inodes per group");
        Ok(Self {
            data,
            block_size: 1024 << log_block_size,
            inode_size,
            inodes_per_group,
            inodes_count: u32_at(sb, 0),
            first_data_block: u64::from(u32_at(sb, 0x14)),
            desc_size,
            incompat,
        })
    }

    /// The `len` bytes at `offset`, if the partition holds them.
    fn bytes(&self, offset: u64, len: usize) -> Result<&'a [u8]> {
        usize::try_from(offset)
            .ok()
            .and_then(|start| self.data.get(start..start.checked_add(len)?))
            .whatever_context("ext4 metadata lies beyond the end of the partition")
    }

    fn block(&self, block: u64) -> Result<&'a [u8]> {
        let offset = block
            .checked_mul(self.block_size as u64)
            .whatever_context("invalid ext4 block number")?;
        self.bytes(offset, self.block_size)
    }

    fn inode(&self, number: u32) -> Result<Inode<'a>> {
        ensure_whatever!(
            (1..=self.inodes_count).contains(&number),
            "invalid ext4 inode number {number}"
        );
        let (group, index) = (
            (number - 1) / self.inodes_per_group,
            (number - 1) % self.inodes_per_group,
        );
        // The group descriptors follow the superblock's block.
        let descriptors = (self.first_data_block + 1) * self.block_size as u64;
        let desc = self.bytes(
            descriptors + u64::from(group) * self.desc_size as u64,
            self.desc_size,
        )?;
        let mut table = u64::from(u32_at(desc, 0x8));
        if self.desc_size >= 0x2c {
            table |= u64::from(u32_at(desc, 0x28)) << 32;
        }
        let offset = table
            .checked_mul(self.block_size as u64)
            .and_then(|t| t.checked_add(u64::from(index) * self.inode_size as u64))
            .whatever_context("invalid ext4 inode table location")?;
        let raw = self.bytes(offset, self.inode_size)?;
        Ok(Inode {
            raw,
            mode: u16_at(raw, 0),
            flags: u32_at(raw, 0x20),
            size: u64::from(u32_at(raw, 0x4)) | (u64::from(u32_at(raw, 0x6c)) << 32),
        })
    }

    /// The physical block of each logical block of `inode`, from its extent tree.
    fn blocks(&self, inode: &Inode<'_>) -> Result<BTreeMap<u64, u64>> {
        ensure_whatever!(
            inode.flags & INLINE_DATA_FL == 0,
            "inline data is not supported"
        );
        ensure_whatever!(
            inode.flags & EXTENTS_FL != 0,
            "block maps without extents are not supported"
        );
        let mut blocks = BTreeMap::new();
        self.extent_node(&inode.raw[0x28..0x28 + 60], MAX_EXTENT_DEPTH, &mut blocks)?;
        Ok(blocks)
    }

    fn extent_node(&self, node: &[u8], max_depth: u16, out: &mut BTreeMap<u64, u64>) -> Result<()> {
        ensure_whatever!(
            node.len() >= 12 && u16_at(node, 0) == EXTENT_MAGIC,
            "invalid ext4 extent header"
        );
        let (entries, depth) = (usize::from(u16_at(node, 2)), u16_at(node, 6));
        ensure_whatever!(depth <= max_depth, "ext4 extent tree is too deep");
        ensure_whatever!(
            12 + entries * 12 <= node.len(),
            "ext4 extent node overflows"
        );
        for e in node[12..12 + entries * 12].chunks(12) {
            if depth == 0 {
                let logical = u64::from(u32_at(e, 0));
                let len = u16_at(e, 4);
                // Longer extents are unwritten: allocated, but read as zeros.
                ensure_whatever!(len <= 32768, "unwritten ext4 extents are not supported");
                let start = u64::from(u32_at(e, 8)) | (u64::from(u16_at(e, 6)) << 32);
                for i in 0..u64::from(len) {
                    ensure_whatever!(
                        out.insert(logical + i, start + i).is_none(),
                        "overlapping ext4 extents"
                    );
                }
            } else {
                let child = u64::from(u32_at(e, 4)) | (u64::from(u16_at(e, 8)) << 32);
                self.extent_node(self.block(child)?, depth - 1, out)?;
            }
        }
        Ok(())
    }

    /// The entries of directory `inode`, other than `.` and `..`, by name.
    fn read_dir(&self, inode: &Inode<'_>) -> Result<Vec<(Vec<u8>, u32)>> {
        let blocks = self.blocks(inode)?;
        let count = inode.size.div_ceil(self.block_size as u64);
        let mut entries = Vec::new();
        for logical in 0..count {
            let physical = blocks
                .get(&logical)
                .whatever_context("ext4 directory has a hole")?;
            let block = self.block(*physical)?;
            // Hash tree nodes and checksum tails are entries of inode 0, which are skipped.
            let mut pos = 0;
            while pos < block.len() {
                ensure_whatever!(pos + 8 <= block.len(), "truncated ext4 directory entry");
                let number = u32_at(block, pos);
                let rec_len = usize::from(u16_at(block, pos + 4));
                let name_len = usize::from(block[pos + 6]);
                ensure_whatever!(
                    rec_len >= 8 && 8 + name_len <= rec_len && pos + rec_len <= block.len(),
                    "invalid ext4 directory entry"
                );
                let name = &block[pos + 8..pos + 8 + name_len];
                if number != 0 && name != b"." && name != b".." {
                    entries.push((name.to_vec(), number));
                }
                pos += rec_len;
            }
        }
        Ok(entries)
    }

    /// The xattrs of `inode`, in its body and in its EA block.
    fn xattrs(&self, inode: &Inode<'_>) -> Result<Xattrs> {
        let mut xattrs = Xattrs::new();
        let raw = inode.raw;
        if raw.len() > GOOD_OLD_INODE_SIZE {
            let start = GOOD_OLD_INODE_SIZE + usize::from(u16_at(raw, 0x80));
            if start + 4 <= raw.len() && u32_at(raw, start) == XATTR_MAGIC {
                // In the inode body, values are relative to the first entry.
                let region = &raw[start + 4..];
                parse_xattrs(region, 0, region, &mut xattrs)?;
            }
        }
        let acl = u64::from(u32_at(raw, 0x68)) | (u64::from(u16_at(raw, 0x76)) << 32);
        if acl != 0 {
            let block = self.block(acl)?;
            ensure_whatever!(
                u32_at(block, 0) == XATTR_MAGIC && u32_at(block, 8) == 1,
                "invalid ext4 xattr block"
            );
            parse_xattrs(block, XATTR_BLOCK_HEADER, block, &mut xattrs)?;
        }
        Ok(xattrs)
    }
}

/// Parse the xattr entries starting at `pos` of `entries`, whose values are in `values`.
fn parse_xattrs(entries: &[u8], mut pos: usize, values: &[u8], out: &mut Xattrs) -> Result<()> {
    // The list ends with four zero bytes, or at the end of the region.
    while pos + 4 <= entries.len() && u32_at(entries, pos) != 0 {
        ensure_whatever!(
            pos + XATTR_ENTRY <= entries.len(),
            "truncated ext4 xattr entry"
        );
        let e = &entries[pos..];
        let (name_len, index) = (usize::from(e[0]), e[1]);
        let value_offset = usize::from(u16_at(e, 2));
        let value_inode = u32_at(e, 4);
        let value_size = usize::try_from(u32_at(e, 8)).expect("32-bit length");
        ensure_whatever!(
            value_inode == 0,
            "ext4 xattr values stored in inodes are not supported"
        );
        let name = e
            .get(XATTR_ENTRY..XATTR_ENTRY + name_len)
            .whatever_context("truncated ext4 xattr name")?;
        let value = values
            .get(value_offset..value_offset + value_size)
            .whatever_context("ext4 xattr value lies beyond its region")?;
        let prefix: &[u8] = match index {
            1 => b"user.",
            2 => b"system.posix_acl_access",
            3 => b"system.posix_acl_default",
            4 => b"trusted.",
            6 => b"security.",
            7 => b"system.",
            8 => b"system.richacl",
            i => {
                snafu::whatever!("unknown ext4 xattr name index {i}");
            }
        };
        fsfiles::insert_unique(out, [prefix, name].concat(), value.to_vec())?;
        pos += (XATTR_ENTRY + name_len).next_multiple_of(4);
    }
    Ok(())
}

/// Every entry of the ext4 filesystem in `partition`, including `/` but not
/// `/lost+found`, keyed by absolute path.
pub fn entries(partition: &[u8]) -> Result<BTreeMap<FsPath, Entry>> {
    let fs = Filesystem::new(partition)?;
    let view = Ext4::load(Box::new(partition.to_vec()))
        .whatever_context("failed to mount ext4 filesystem")?;
    let mut entries = BTreeMap::new();
    let mut visited = BTreeSet::new();
    let mut stack = vec![(b"/".to_vec(), ROOT_INODE)];
    while let Some((path, number)) = stack.pop() {
        let name = display(&path);
        let inode = fs
            .inode(number)
            .with_whatever_context(|_| format!("failed to read the inode of {name}"))?;
        let kind = Kind::from_mode(inode.mode)
            .with_whatever_context(|| format!("{name} has an invalid mode"))?;
        if kind == Kind::Directory {
            ensure_whatever!(
                visited.insert(number),
                "directory {name} is linked more than once"
            );
            let children = fs
                .read_dir(&inode)
                .with_whatever_context(|_| format!("failed to read directory {name}"))?;
            for (child, child_number) in children {
                if path == b"/" && child == b"lost+found" {
                    continue;
                }
                let mut child_path = path.clone();
                if path != b"/" {
                    child_path.push(b'/');
                }
                child_path.extend_from_slice(&child);
                stack.push((child_path, child_number));
            }
        }
        let device = match inode.mode & S_IFMT {
            S_IFCHR | S_IFBLK => {
                // Old-style numbers are in the first block slot, new-style in the second.
                let old = u32_at(inode.raw, 0x28);
                Some(if old != 0 {
                    ((old >> 8) & 0xff, old & 0xff)
                } else {
                    fsfiles::decode_device(u32_at(inode.raw, 0x2c))
                })
            }
            _ => None,
        };
        let xattrs = fs
            .xattrs(&inode)
            .with_whatever_context(|_| format!("failed to read the xattrs of {name}"))?;
        let mut entry = Entry {
            kind,
            mode: u32::from(inode.mode & 0o7777),
            uid: u32::from(u16_at(inode.raw, 0x2)) | (u32::from(u16_at(inode.raw, 0x78)) << 16),
            gid: u32::from(u16_at(inode.raw, 0x18)) | (u32::from(u16_at(inode.raw, 0x7a)) << 16),
            link: None,
            xattrs,
            size: if kind == Kind::File { inode.size } else { 0 },
            device,
        };
        cross_check(&view, &path, &mut entry)
            .with_whatever_context(|_| format!("failed to read the metadata of {name}"))?;
        fsfiles::insert_unique(&mut entries, path, entry)?;
    }
    Ok(entries)
}

/// Check `entry` against what `ext4_view` reads at `path`, and take its symlink target.
fn cross_check(view: &Ext4, path: &[u8], entry: &mut Entry) -> Result<()> {
    let view_path = ext4_view::Path::try_from(path).whatever_context("invalid path")?;
    let metadata = view
        .symlink_metadata(view_path)
        .whatever_context("ext4_view could not stat it")?;
    let kind = match metadata.file_type() {
        FileType::BlockDevice => Kind::BlockDevice,
        FileType::CharacterDevice => Kind::CharDevice,
        FileType::Directory => Kind::Directory,
        FileType::Fifo => Kind::Fifo,
        FileType::Regular => Kind::File,
        FileType::Socket => Kind::Socket,
        FileType::Symlink => Kind::Symlink,
    };
    let size = if kind == Kind::File {
        metadata.len()
    } else {
        0
    };
    ensure_whatever!(
        (
            kind,
            u32::from(metadata.mode()),
            metadata.uid(),
            metadata.gid(),
            size
        ) == (entry.kind, entry.mode, entry.uid, entry.gid, entry.size),
        "its inode and ext4_view disagree on its type, mode, owner or size"
    );
    if kind == Kind::Symlink {
        let target = view
            .read_link(view_path)
            .whatever_context("failed to read its symlink target")?;
        entry.link = Some(target.as_ref().to_vec());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xattr_entries() {
        // A security.selinux entry, then the end of the list.
        let mut region = vec![7, 6, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0];
        region.extend(b"selinux\0");
        region.extend([0; 4]);
        let value_offset = region.len() as u16;
        region[2..4].copy_from_slice(&value_offset.to_le_bytes());
        region.extend(b"lbl");
        let mut xattrs = Xattrs::new();
        parse_xattrs(&region, 0, &region, &mut xattrs).unwrap();
        assert_eq!(xattrs[&b"security.selinux".to_vec()], b"lbl");

        // Values out of range, in inodes, or of unknown namespaces fail.
        for (at, value) in [(8, 200u8), (4, 1), (1, 9)] {
            let mut bad = region.clone();
            bad[at] = value;
            assert!(parse_xattrs(&bad, 0, &bad, &mut Xattrs::new()).is_err());
        }
    }

    /// An ext4 image of `src` as mkfs.ext4 writes it, then changed by `debugfs` commands.
    fn mkfs(dir: &std::path::Path, src: &std::path::Path, debugfs: &[&str]) -> Vec<u8> {
        use std::process::Command;
        let img = dir.join("ext4.fs");
        let _ = std::fs::remove_file(&img);
        let output = Command::new("mkfs.ext4")
            .env("E2FSPROGS_FAKE_TIME", "1")
            .args([
                "-q",
                "-b",
                "1024",
                "-E",
                "root_owner=0:0,no_copy_xattrs",
                "-d",
            ])
            .arg(src)
            .arg(&img)
            .arg("10M")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        for command in debugfs {
            let output = Command::new("debugfs")
                .args(["-w", "-R", command])
                .arg(&img)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        std::fs::read(img).unwrap()
    }

    #[test]
    #[ignore = "needs mkfs.ext4"]
    fn reads_entries_like_debugfs_sets_them() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("d")).unwrap();
        std::fs::write(src.join("d/file"), "x").unwrap();
        std::os::unix::fs::symlink("d/file", src.join("link")).unwrap();
        // Enough files that the directory is indexed with a hash tree.
        for i in 0..200 {
            std::fs::write(src.join(format!("d/many-{i:03}")), "").unwrap();
        }
        let big = "v".repeat(300);
        let fs = mkfs(
            dir.path(),
            &src,
            &[
                "ea_set /d/file security.selinux system_u:object_r:etc_t:s0",
                &format!("ea_set /d/file user.big {big}"),
                "ea_set / security.selinux system_u:object_r:root_t:s0",
                // Relative to debugfs's working directory, the root.
                "mknod console c 5 1",
                "sif /d/file uid 70000",
            ],
        );
        let entries = entries(&fs).unwrap();
        assert!(!entries.contains_key(b"/lost+found".as_slice()));
        assert_eq!(entries.len(), 205);
        let file = &entries[b"/d/file".as_slice()];
        assert_eq!(file.kind, Kind::File);
        assert_eq!(file.uid, 70000);
        assert_eq!(file.size, 1);
        assert_eq!(
            file.xattrs[b"security.selinux".as_slice()],
            b"system_u:object_r:etc_t:s0"
        );
        assert_eq!(file.xattrs[b"user.big".as_slice()], big.as_bytes());
        assert_eq!(
            entries[b"/link".as_slice()].link.as_deref(),
            Some(&b"d/file"[..])
        );
        let console = &entries[b"/console".as_slice()];
        assert_eq!(console.kind, Kind::CharDevice);
        assert_eq!(console.device, Some((5, 1)));
        assert!(!needs_recovery(&fs).unwrap());
    }
}
