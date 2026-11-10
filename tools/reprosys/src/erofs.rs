//! Reading erofs root filesystems with erofs-utils. The entries come from
//! `dump.erofs` listings and their inodes; `fsck.erofs` extracts the files'
//! contents and symlink targets. Long xattr name prefixes and metaboxes are not
//! supported.

use crate::error::Result;
use crate::fsfiles::{self, display, Entry, FileTree, FsPath, Kind, Xattrs};
use regex::bytes::Regex;
use snafu::{ensure_whatever, OptionExt, ResultExt};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, Permissions};
use std::io::Read;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use tempfile::TempDir;

const SUPERBLOCK_OFFSET: u64 = 1024;
const MAGIC: u32 = 0xe0f5_e1e2;
const INCOMPAT_METABOX: u32 = 0x100;
/// Inode numbers (NIDs) count 32-byte slots from the start of the metadata area.
const INODE_SLOT_BITS: u32 = 5;
const XATTR_IBODY_HEADER_SIZE: usize = 12;
const XATTR_ENTRY_SIZE: usize = 4;
const XATTR_LONG_PREFIX: u8 = 0x80;
const S_IFMT: u16 = 0xf000;
const S_IFCHR: u16 = 0x2000;
const S_IFBLK: u16 = 0x6000;
/// The file type of directory entries for directories.
const FT_DIR: u8 = 2;

/// Whether the partition at `offset` of `file` holds an erofs filesystem.
pub fn is_erofs(file: &File, offset: u64) -> Result<bool> {
    let mut magic = [0u8; 4];
    match file.read_exact_at(&mut magic, offset + SUPERBLOCK_OFFSET) {
        Ok(()) => Ok(u32::from_le_bytes(magic) == MAGIC),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e).whatever_context("failed to read filesystem superblock"),
    }
}

/// The total size of the regular files in the erofs filesystem at `offset`.
pub fn total_file_size(image: &Path, offset: u64) -> Result<u64> {
    let output = Command::new("dump.erofs")
        .arg(format!("--offset={offset}"))
        .arg("-S")
        .arg(image)
        .stdin(Stdio::null())
        .output()
        .whatever_context("failed to run dump.erofs")?;
    parse_total_file_size(&String::from_utf8_lossy(&output.stdout)).whatever_context(format!(
        "dump.erofs did not report the size of the files in {}: {}",
        image.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// Parse the `Filesystem total original file size: <n> Bytes` line of `dump.erofs -S` output.
fn parse_total_file_size(output: &str) -> Option<u64> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Filesystem total original file size:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

/// The fields of the erofs superblock needed to find inodes and shared xattrs.
struct Superblock {
    meta_start: u64,
    xattr_start: u64,
}

/// A `dump.erofs --ls` listing: the directory's inode and its entries.
#[derive(Debug, PartialEq, Eq)]
struct Listing {
    nid: u64,
    entries: Vec<(Vec<u8>, u64, u8)>,
}

/// The attributes read from an inode in the image.
struct RawInode {
    mode: u16,
    uid: u32,
    gid: u32,
    size: u64,
    xattrs: Xattrs,
}

/// The kind of entry a directory entry's file type describes.
fn file_type_kind(file_type: u8) -> Option<Kind> {
    Some(match file_type {
        1 => Kind::File,
        FT_DIR => Kind::Directory,
        3 => Kind::CharDevice,
        4 => Kind::BlockDevice,
        5 => Kind::Fifo,
        6 => Kind::Socket,
        7 => Kind::Symlink,
        _ => return None,
    })
}

/// The kind of an extracted entry.
fn extracted_kind(file_type: fs::FileType) -> Kind {
    if file_type.is_file() {
        Kind::File
    } else if file_type.is_dir() {
        Kind::Directory
    } else if file_type.is_symlink() {
        Kind::Symlink
    } else if file_type.is_char_device() {
        Kind::CharDevice
    } else if file_type.is_block_device() {
        Kind::BlockDevice
    } else if file_type.is_fifo() {
        Kind::Fifo
    } else {
        Kind::Socket
    }
}

/// The kind of entry an inode's mode describes.
fn mode_kind(mode: u16) -> Option<Kind> {
    Some(match mode & S_IFMT {
        0x8000 => Kind::File,
        0x4000 => Kind::Directory,
        0xa000 => Kind::Symlink,
        S_IFCHR => Kind::CharDevice,
        S_IFBLK => Kind::BlockDevice,
        0x1000 => Kind::Fifo,
        0xc000 => Kind::Socket,
        _ => return None,
    })
}

/// An erofs filesystem extracted to a temporary directory.
pub struct ErofsTree {
    image: PathBuf,
    offset: u64,
    len: u64,
    dir: TempDir,
}

impl ErofsTree {
    /// Extract the erofs filesystem at `offset` of `image` below `work_dir`.
    pub fn extract(image: &Path, offset: u64, len: u64, work_dir: &Path) -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("erofs-")
            .tempdir_in(work_dir)
            .whatever_context(format!(
                "failed to create a directory in {}",
                work_dir.display()
            ))?;
        let tree = Self {
            image: image.to_path_buf(),
            offset,
            len,
            dir,
        };
        let mut extract = OsString::from("--extract=");
        extract.push(tree.root());
        let output = Command::new("fsck.erofs")
            .arg(format!("--offset={offset}"))
            .arg(extract)
            .args(["--preserve-perms", "--no-preserve-owner"])
            .arg(image)
            .stdin(Stdio::null())
            .output()
            .whatever_context("failed to run fsck.erofs")?;
        if !output.status.success() {
            // Best effort: read-only directories extracted so far would block cleanup.
            let _ = make_accessible(tree.dir.path());
            snafu::whatever!(
                "fsck.erofs failed to extract {}: {}",
                image.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        // Extracted entries keep their permissions, which can be read-only.
        make_accessible(tree.dir.path())
            .whatever_context(format!("failed to make {} readable", tree.root().display()))?;
        Ok(tree)
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    /// List the directory `path` (absolute within the filesystem) with `dump.erofs`.
    fn list_dir(&self, path: &[u8]) -> Result<Listing> {
        let mut arg = OsString::from("--path=");
        arg.push(std::ffi::OsStr::from_bytes(path));
        let output = Command::new("dump.erofs")
            .arg(format!("--offset={}", self.offset))
            .arg("--ls")
            .arg(arg)
            .arg(&self.image)
            .stdin(Stdio::null())
            .output()
            .whatever_context("failed to run dump.erofs")?;
        // dump.erofs exits zero even when the lookup fails, so rely on its output.
        parse_listing(&output.stdout).whatever_context(format!(
            "dump.erofs did not list directory {}: {}",
            display(path),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }

    /// List the directories `paths`, running `dump.erofs` in parallel.
    fn list_dirs(&self, paths: &[Vec<u8>]) -> Result<Vec<Listing>> {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(16))
            .unwrap_or(4);
        let chunk = paths.len().div_ceil(threads).max(1);
        std::thread::scope(|s| {
            let handles: Vec<_> = paths
                .chunks(chunk)
                .map(|chunk| {
                    s.spawn(move || {
                        // Errors are not `Send`, so pass them back as messages.
                        chunk
                            .iter()
                            .map(|p| self.list_dir(p).map_err(|e| e.to_string()))
                            .collect::<std::result::Result<Vec<_>, String>>()
                    })
                })
                .collect();
            let mut listings = Vec::with_capacity(paths.len());
            for handle in handles {
                match handle.join().expect("dump.erofs worker panicked") {
                    Ok(chunk) => listings.extend(chunk),
                    Err(e) => {
                        snafu::whatever!("{e}");
                    }
                }
            }
            Ok(listings)
        })
    }

    fn read_at(&self, file: &File, pos: u64, len: usize) -> Result<Vec<u8>> {
        let mut out = vec![0u8; len];
        pos.checked_add(len as u64)
            .filter(|end| *end <= self.len)
            .whatever_context("erofs metadata lies beyond the end of the partition")?;
        file.read_exact_at(&mut out, self.offset + pos)
            .whatever_context(format!("failed to read {}", self.image.display()))?;
        Ok(out)
    }

    fn superblock(&self, file: &File) -> Result<Superblock> {
        let sb = self.read_at(file, SUPERBLOCK_OFFSET, 128)?;
        let u32_at = |o: usize| u32::from_le_bytes(sb[o..o + 4].try_into().expect("4 bytes"));
        ensure_whatever!(u32_at(0) == MAGIC, "not an erofs filesystem");
        ensure_whatever!(
            u32_at(80) & INCOMPAT_METABOX == 0,
            "erofs filesystems with a metabox are not supported"
        );
        let block_bits = u32::from(sb[12]);
        ensure_whatever!(
            (9..=16).contains(&block_bits),
            "unsupported erofs block size 2^{block_bits}"
        );
        Ok(Superblock {
            meta_start: u64::from(u32_at(40)) << block_bits,
            xattr_start: u64::from(u32_at(44)) << block_bits,
        })
    }

    /// Read inode `nid`, in its compact (32-byte) or extended (64-byte) form.
    fn inode(&self, file: &File, sb: &Superblock, nid: u64) -> Result<RawInode> {
        let inode_pos = nid
            .checked_mul(1 << INODE_SLOT_BITS)
            .and_then(|p| p.checked_add(sb.meta_start))
            .whatever_context(format!("location of erofs inode {nid} overflows"))?;
        let header = self.read_at(file, inode_pos, 2)?;
        let extended = u16::from_le_bytes([header[0], header[1]]) & 1 != 0;
        let inode_size = if extended { 64 } else { 32 };
        let inode = self.read_at(file, inode_pos, inode_size)?;
        let u16_at = |o: usize| u16::from_le_bytes([inode[o], inode[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes(inode[o..o + 4].try_into().expect("4 bytes"));
        let mode = u16_at(4);
        let (uid, gid, size) = if extended {
            let size = u64::from_le_bytes(inode[8..16].try_into().expect("8 bytes"));
            (u32_at(24), u32_at(28), size)
        } else {
            (
                u32::from(u16_at(24)),
                u32::from(u16_at(26)),
                u64::from(u32_at(8)),
            )
        };
        let icount = usize::from(u16_at(2));
        let xattrs = self.xattrs(file, sb, nid, inode_pos + inode_size as u64, icount)?;
        Ok(RawInode {
            mode,
            uid,
            gid,
            size,
            xattrs,
        })
    }

    /// Read the shared, then inline, xattrs of inode `nid`, starting at `pos`.
    fn xattrs(
        &self,
        file: &File,
        sb: &Superblock,
        nid: u64,
        pos: u64,
        icount: usize,
    ) -> Result<Xattrs> {
        let mut xattrs = Xattrs::new();
        if icount == 0 {
            return Ok(xattrs);
        }
        // As in the kernel: a header followed by (icount - 1) 4-byte slots.
        let size = XATTR_IBODY_HEADER_SIZE + (icount - 1) * 4;
        ensure_whatever!(
            size > XATTR_IBODY_HEADER_SIZE,
            "unsupported xattr layout in erofs inode {nid}"
        );
        let body = self.read_at(file, pos, size)?;
        let shared = usize::from(body[4]);
        let inline_start = XATTR_IBODY_HEADER_SIZE + shared * 4;
        ensure_whatever!(
            inline_start <= size,
            "corrupt shared xattr count in erofs inode {nid}"
        );
        for id in body[XATTR_IBODY_HEADER_SIZE..inline_start]
            .as_chunks::<4>()
            .0
        {
            let id = u64::from(u32::from_le_bytes(*id));
            let pos = sb.xattr_start + id * 4;
            let entry = self.read_at(file, pos, XATTR_ENTRY_SIZE)?;
            let len = XATTR_ENTRY_SIZE
                + usize::from(entry[0])
                + usize::from(u16::from_le_bytes([entry[2], entry[3]]));
            let entry = self.read_at(file, pos, len)?;
            parse_xattr(&entry, &mut xattrs)?;
        }
        let mut pos = inline_start;
        while pos < size {
            let len = parse_xattr(&body[pos..], &mut xattrs)?;
            pos += len.next_multiple_of(4);
        }
        Ok(xattrs)
    }
}

/// Parse one erofs xattr entry at the start of `data` into `out`, and return its length.
fn parse_xattr(data: &[u8], out: &mut Xattrs) -> Result<usize> {
    ensure_whatever!(
        data.len() >= XATTR_ENTRY_SIZE,
        "truncated erofs xattr entry"
    );
    let name_len = usize::from(data[0]);
    let index = data[1];
    let value_len = usize::from(u16::from_le_bytes([data[2], data[3]]));
    let len = XATTR_ENTRY_SIZE + name_len + value_len;
    ensure_whatever!(data.len() >= len, "truncated erofs xattr entry");
    let prefix: &[u8] = match index {
        1 => b"user.",
        2 => b"system.posix_acl_access",
        3 => b"system.posix_acl_default",
        4 => b"trusted.",
        5 => b"lustre.",
        6 => b"security.",
        i if i & XATTR_LONG_PREFIX != 0 => {
            snafu::whatever!("erofs long xattr name prefixes are not supported");
        }
        i => {
            snafu::whatever!("unknown erofs xattr name index {i}");
        }
    };
    let mut name = prefix.to_vec();
    name.extend_from_slice(&data[XATTR_ENTRY_SIZE..XATTR_ENTRY_SIZE + name_len]);
    let value = data[XATTR_ENTRY_SIZE + name_len..len].to_vec();
    fsfiles::insert_unique(out, name, value)?;
    Ok(len)
}

/// Make `dir` and everything below it readable and removable by its owner.
fn make_accessible(dir: &Path) -> std::io::Result<()> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mode = fs::symlink_metadata(&dir)?.mode();
        fs::set_permissions(&dir, Permissions::from_mode((mode & 0o7777) | 0o700))?;
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                let mode = entry.metadata()?.mode();
                fs::set_permissions(
                    entry.path(),
                    Permissions::from_mode((mode & 0o7777) | 0o400),
                )?;
            }
        }
    }
    Ok(())
}

/// The line of `dump.erofs --ls` output that precedes the directory entries.
const LISTING_HEADER: &[u8] = b"       NID TYPE  FILENAME\n";

/// A directory entry line of `dump.erofs --ls` output: `%10llu %4u  <name>`.
static LISTING_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s-u)^ *([0-9]+) +([0-9]+)  (.*)$").expect("valid regex"));

/// Parse `dump.erofs --ls` output. A line that is not an entry continues the
/// previous entry's name; a misread name fails the comparison.
fn parse_listing(output: &[u8]) -> Option<Listing> {
    let start = output
        .windows(LISTING_HEADER.len())
        .position(|w| w == LISTING_HEADER)?;
    let mut tokens = std::str::from_utf8(&output[..start])
        .ok()?
        .split_whitespace();
    let mut nid = None;
    while let Some(token) = tokens.next() {
        if token == "NID:" {
            nid = tokens.next()?.parse().ok();
        }
    }
    let body = &output[start + LISTING_HEADER.len()..];
    let mut entries: Vec<(Vec<u8>, u64, u8)> = Vec::new();
    if let Some(body) = body.strip_suffix(b"\n") {
        for line in body.split(|b| *b == b'\n') {
            match LISTING_LINE_RE.captures(line) {
                Some(c) => {
                    let nid = std::str::from_utf8(&c[1]).ok()?.parse().ok()?;
                    let file_type = std::str::from_utf8(&c[2]).ok()?.parse().ok()?;
                    entries.push((c[3].to_vec(), nid, file_type));
                }
                None => {
                    let (name, ..) = entries.last_mut()?;
                    name.push(b'\n');
                    name.extend_from_slice(line);
                }
            }
        }
    }
    entries.retain(|(name, ..)| name != b"." && name != b"..");
    Some(Listing { nid: nid?, entries })
}

impl FileTree for ErofsTree {
    type Handle = PathBuf;

    fn entries(&self) -> Result<BTreeMap<FsPath, (PathBuf, Entry)>> {
        // List the image breadth-first, a level of directories at a time.
        let mut listed = Vec::new();
        let mut level = vec![b"/".to_vec()];
        while !level.is_empty() {
            let mut next = Vec::new();
            for (dir, listing) in level.iter().zip(self.list_dirs(&level)?) {
                if dir == b"/" {
                    listed.push((dir.clone(), listing.nid, FT_DIR));
                }
                for (name, nid, file_type) in listing.entries {
                    let mut fs_path = dir.clone();
                    if fs_path != b"/" {
                        fs_path.push(b'/');
                    }
                    fs_path.extend_from_slice(&name);
                    if file_type == FT_DIR {
                        next.push(fs_path.clone());
                    }
                    listed.push((fs_path, nid, file_type));
                }
            }
            level = next;
        }

        let image = File::open(&self.image)
            .whatever_context(format!("failed to open {}", self.image.display()))?;
        let sb = self.superblock(&image)?;
        let root = self.root();
        let mut entries = BTreeMap::new();
        for (fs_path, nid, file_type) in listed {
            let name = display(&fs_path);
            let kind = file_type_kind(file_type).with_whatever_context(|| {
                format!("{name} has unknown erofs file type {file_type}")
            })?;
            let inode = self
                .inode(&image, &sb, nid)
                .with_whatever_context(|_| format!("failed to read the inode of {name}"))?;
            ensure_whatever!(
                mode_kind(inode.mode) == Some(kind),
                "the inode of {name} is not a {}",
                kind.name()
            );
            let path = root.join(OsStr::from_bytes(&fs_path[1..]));
            let extracted = fs::symlink_metadata(&path).map(|m| extracted_kind(m.file_type()));
            let link = match kind {
                // Bottlerocket root filesystems hold none, and extraction cannot create them.
                Kind::CharDevice | Kind::BlockDevice => {
                    snafu::whatever!("{name} is a {}, which is not supported", kind.name());
                }
                _ if extracted.ok() != Some(kind) => {
                    snafu::whatever!("{name} is in the erofs image but was not extracted");
                }
                Kind::Symlink => {
                    let target = fs::read_link(&path)
                        .whatever_context(format!("failed to read symlink {}", path.display()))?;
                    Some(target.into_os_string().into_vec())
                }
                _ => None,
            };
            let entry = Entry {
                kind,
                mode: u32::from(inode.mode & 0o7777),
                uid: inode.uid,
                gid: inode.gid,
                link,
                xattrs: inode.xattrs,
                size: if kind == Kind::File { inode.size } else { 0 },
            };
            fsfiles::insert_unique(&mut entries, fs_path, (path, entry))?;
        }

        Ok(entries)
    }

    fn open(&self, handle: &PathBuf) -> Result<Box<dyn Read + '_>> {
        let file =
            File::open(handle).whatever_context(format!("failed to open {}", handle.display()))?;
        Ok(Box::new(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dump_output() {
        let output = b"Path : /d\nSize: 69  On-disk size: 69  directory\n\
                       NID: 39   Links: 2   Layout: 2   Compression ratio: 100.00%\n\
                       Uid: 0   Gid: 0  Access: 0755/rwxr-xr-x\n\n\
                       \x20      NID TYPE  FILENAME\n\
                       \x20       39    2  .\n\
                       \x20       36    2  ..\n\
                       \x20       44    3  c\n\
                       \x20       46    1  a b\n\
                       \x20       48    1  x\n\
                       nl\xff\n";
        assert_eq!(
            parse_listing(output),
            Some(Listing {
                nid: 39,
                entries: vec![
                    (b"c".to_vec(), 44, 3),
                    (b"a b".to_vec(), 46, 1),
                    (b"x\nnl\xff".to_vec(), 48, 1),
                ]
            })
        );
        assert_eq!(parse_listing(b""), None);

        let stats = "Filesystem regular file count:\t\t2\n\
                     Filesystem total original file size:    6 Bytes\n\
                     Filesystem total file size:             6 Bytes\n";
        assert_eq!(parse_total_file_size(stats), Some(6));
        assert_eq!(parse_total_file_size(""), None);
    }

    #[test]
    fn parses_xattr_entries() {
        let mut xattrs = Xattrs::new();
        let entry = [
            7, 6, 3, 0, b's', b'e', b'l', b'i', b'n', b'u', b'x', b'l', b'b', b'l',
        ];
        assert_eq!(parse_xattr(&entry, &mut xattrs).unwrap(), entry.len());
        assert_eq!(xattrs[&b"security.selinux".to_vec()], b"lbl");
        // Truncated values, long prefixes and duplicates are errors.
        assert!(parse_xattr(&entry[..12], &mut Xattrs::new()).is_err());
        assert!(parse_xattr(&[1, 0x81, 0, 0, b'x'], &mut Xattrs::new()).is_err());
        assert!(parse_xattr(&entry, &mut xattrs).is_err());
    }

    #[test]
    fn detects_erofs_magic() {
        let f = tempfile::tempfile().unwrap();
        f.write_all_at(&MAGIC.to_le_bytes(), 4096 + SUPERBLOCK_OFFSET)
            .unwrap();
        f.set_len(8192).unwrap();
        assert!(is_erofs(&f, 4096).unwrap());
        assert!(!is_erofs(&f, 0).unwrap());
        assert!(!is_erofs(&f, 1 << 20).unwrap());
    }
}
