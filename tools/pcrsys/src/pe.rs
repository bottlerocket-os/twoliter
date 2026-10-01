//! PE file parsing and Authenticode hash calculation.
//!
//! This matches the behavior of `pesign --hash`.

use crate::error::Result;
use authenticode::authenticode_digest;
use object::read::pe::PeFile;
use object::{pe, Object, ObjectSection};
use sha2::{Digest, Sha256};
use snafu::{ensure_whatever, whatever, OptionExt};
use std::borrow::Cow;

/// Calculate Authenticode SHA256 hash of a PE32+ file.
/// Pads to 8-byte boundary to match pesign behavior.
pub fn get_authenticode_hash(pe_data: &[u8]) -> Result<[u8; 32]> {
    let padding = (8 - (pe_data.len() % 8)) % 8;
    let data: Cow<[u8]> = if padding > 0 {
        let mut padded = pe_data.to_vec();
        padded.resize(pe_data.len() + padding, 0);
        padded.into()
    } else {
        pe_data.into()
    };

    let pe = PeFile::<pe::ImageNtHeaders64>::parse(&data[..])
        .ok()
        .whatever_context("failed to parse PE32+ file")?;

    let mut hasher = Sha256::new();
    ensure_whatever!(
        authenticode_digest(&pe, &mut hasher).is_ok(),
        "authenticode digest failed"
    );
    Ok(hasher.finalize().into())
}

/// Extract vendor certificate from shim's `.vendor_cert` section.
///
/// Returns the DER-encoded X.509 certificate.
///
/// The section contains a header with certificate offsets and sizes:
/// - bytes 0-3: vendor_authorized_size (u32 LE)
/// - bytes 4-7: vendor_deauthorized_size (u32 LE)
/// - bytes 8-11: vendor_authorized_offset (u32 LE)
/// - bytes 12-15: vendor_deauthorized_offset (u32 LE)
/// - followed by certificate data
pub fn extract_vendor_cert(pe_data: &[u8]) -> Result<Vec<u8>> {
    let section_data = get_section_data(pe_data, ".vendor_cert")?;
    ensure_whatever!(
        section_data.len() >= 16,
        ".vendor_cert section too small for header"
    );

    let authorized_size = u32::from_le_bytes(section_data[0..4].try_into().unwrap()) as usize;
    let authorized_offset = u32::from_le_bytes(section_data[8..12].try_into().unwrap()) as usize;

    ensure_whatever!(authorized_size > 0, "no vendor certificate present");
    ensure_whatever!(
        authorized_offset
            .checked_add(authorized_size)
            .is_some_and(|end| end <= section_data.len()),
        "vendor certificate extends beyond section"
    );

    Ok(section_data[authorized_offset..authorized_offset + authorized_size].to_vec())
}

/// Extract SBAT level from shim's `.sbatlevel` section.
///
/// Returns the first (automatic) SBAT level string, which is what shim measures to PCR 7.
///
/// The section contains a 12-byte header followed by null-terminated SBAT strings:
/// - bytes 0-3: unused (u32 LE)
/// - bytes 4-7: offset to first string (u32 LE)
/// - bytes 8-11: unused (u32 LE)
/// - bytes 12+: null-terminated SBAT strings
pub fn extract_sbat_level(pe_data: &[u8]) -> Result<Vec<u8>> {
    const HEADER_SIZE: usize = 12;

    let section_data = get_section_data(pe_data, ".sbatlevel")?;
    ensure_whatever!(
        section_data.len() > HEADER_SIZE,
        ".sbatlevel section too small for header"
    );

    // Skip 12-byte header, read until first null
    let end = section_data[HEADER_SIZE..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| HEADER_SIZE + p);

    let end = end.whatever_context("SBAT level string not null-terminated")?;
    ensure_whatever!(end > HEADER_SIZE, "empty SBAT level string");

    Ok(section_data[HEADER_SIZE..end].to_vec())
}

/// Determine whether a PE image is a Unified Kernel Image (UKI).
///
/// A UKI embeds the kernel in a `.linux` PE section (systemd-stub); shim and
/// other chain loaders do not have one.
pub fn is_uki(pe_data: &[u8]) -> bool {
    get_section_data(pe_data, ".linux").is_ok()
}

/// Extract the kernel command line from a UKI's `.cmdline` PE section.
pub fn extract_uki_cmdline(pe_data: &[u8]) -> Result<String> {
    let section_data = get_section_data(pe_data, ".cmdline")?;

    // Strip trailing NUL padding.
    let end = section_data
        .iter()
        .rposition(|&b| b != 0)
        .map(|p| p + 1)
        .unwrap_or(0);

    let cmdline = std::str::from_utf8(&section_data[..end])
        .ok()
        .whatever_context(".cmdline section is not valid UTF-8")?
        .to_string();

    Ok(cmdline)
}

/// Get raw data from a named PE section.
fn get_section_data<'a>(pe_data: &'a [u8], name: &str) -> Result<&'a [u8]> {
    let pe = PeFile::<pe::ImageNtHeaders64>::parse(pe_data)
        .ok()
        .whatever_context("failed to parse PE32+ file")?;

    for section in pe.sections() {
        if section.name() == Ok(name) {
            let data = section
                .data()
                .ok()
                .whatever_context("failed to read section data")?;
            return Ok(data);
        }
    }

    whatever!("section '{name}' not found");
}

/// UTF-16LE `LoadOptions` bytes the Linux EFI stub measures into PCR 9.
///
/// systemd-stub passes the `.cmdline` section to the kernel verbatim as the EFI
/// `LoadOptions` (UTF-16LE, NUL-terminated); the stub measures exactly those
/// bytes (event `LOADED_IMAGE::LoadOptions`).
pub fn uki_cmdline_load_options(pe_data: &[u8]) -> Result<Vec<u8>> {
    let cmdline = extract_uki_cmdline(pe_data)?;
    let mut load_options: Vec<u8> = cmdline
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    load_options.extend_from_slice(&[0, 0]); // UTF-16 NUL terminator, also measured
    Ok(load_options)
}

/// Pad `buf` with NULs until its length since `start` is a multiple of 4;
/// `newc` aligns every record (and file data) to a 4-byte boundary.
fn pad4(buf: &mut Vec<u8>, start: usize) {
    while !(buf.len() - start).is_multiple_of(4) {
        buf.push(0);
    }
}

/// Write a CPIO `newc` header (magic + 13 8-hex-digit fields). Every record we
/// emit uses uid/gid/mtime/dev*/crc = 0 and nlink = 1, so only `inode`, `mode`,
/// `filesize` and `namesize` vary.
fn cpio_header(buf: &mut Vec<u8>, inode: u32, mode: u32, filesize: u32, namesize: u32) {
    buf.extend_from_slice(b"070701");
    // inode, mode, uid, gid, nlink, mtime, filesize, dev{major,minor},
    // rdev{major,minor}, namesize, crc
    let fields = [inode, mode, 0, 0, 1, 0, filesize, 0, 0, 0, 0, namesize, 0];
    for f in fields {
        buf.extend_from_slice(format!("{f:08x}").as_bytes());
    }
}

/// CPIO `newc` directory inode (systemd-stub `pack_cpio_dir`).
fn cpio_dir(buf: &mut Vec<u8>, path: &str, access_mode: u32, inode: u32) {
    let start = buf.len();
    cpio_header(buf, inode, access_mode | 0o040000, 0, path.len() as u32 + 1);
    buf.extend_from_slice(path.as_bytes());
    buf.push(0);
    pad4(buf, start);
}

/// CPIO `newc` regular-file inode (systemd-stub `pack_cpio_one`), named
/// `<dir_prefix>/<filename>`.
fn cpio_file(
    buf: &mut Vec<u8>,
    dir_prefix: &str,
    filename: &str,
    data: &[u8],
    access_mode: u32,
    inode: u32,
) {
    let start = buf.len();
    let namesize = dir_prefix.len() as u32 + filename.len() as u32 + 2; // '/' + NUL
    cpio_header(
        buf,
        inode,
        access_mode | 0o100000,
        data.len() as u32,
        namesize,
    );
    buf.extend_from_slice(dir_prefix.as_bytes());
    buf.push(b'/');
    buf.extend_from_slice(filename.as_bytes());
    buf.push(0);
    pad4(buf, start);
    buf.extend_from_slice(data);
    pad4(buf, start);
}

/// CPIO `newc` archive trailer (systemd-stub `pack_cpio_trailer`), measured
/// verbatim. Kept as a literal rather than built via [`cpio_header`] because
/// systemd emits the trailer's `namesize` in UPPERCASE hex (`0000000B`) while
/// our other headers use lowercase; a live-TPM check confirmed the uppercase
/// form. The four NUL bytes after `TRAILER!!!` are its name NUL plus padding to
/// a 4-byte boundary.
const CPIO_TRAILER: &[u8] = concat!(
    "070701",   // magic
    "00000000", // inode
    "00000000", // mode
    "00000000", // uid
    "00000000", // gid
    "00000001", // nlink
    "00000000", // mtime
    "00000000", // filesize
    "00000000", // devmajor
    "00000000", // devminor
    "00000000", // rdevmajor
    "00000000", // rdevminor
    "0000000B", // namesize = 11 ("TRAILER!!!\0"), uppercase per systemd
    "00000000", // crc
    "TRAILER!!!",
    "\0\0\0\0",
)
.as_bytes();

/// The cpio systemd-stub's `pack_cpio_literal()` produces for one embedded
/// section: the `.extra` dir (0555), the file `.extra/<filename>` (0444), and
/// the trailer.
fn build_extra_cpio(filename: &str, data: &[u8]) -> Vec<u8> {
    let mut cpio = Vec::new();
    cpio_dir(&mut cpio, ".extra", 0o555, 1);
    cpio_file(&mut cpio, ".extra", filename, data, 0o444, 2);
    cpio.extend_from_slice(CPIO_TRAILER);
    cpio
}

/// Reconstruct the initrd byte stream a direct-UKI boot hands the kernel, which
/// the Linux EFI stub hashes into PCR 9 (event `Linux initrd`).
///
/// systemd-stub embeds `.ucode`, `.initrd`, `.pcrsig`, `.pcrpkey`, `.osrel` and
/// `.profile`. In a Bottlerocket UKI all but `.osrel` is empty, so we use only
/// that for calculation. Any change in any of the above sections would break
/// PCR9 calculations.
pub fn build_uki_synthetic_initrd(pe_data: &[u8]) -> Result<Vec<u8>> {
    match get_section_data(pe_data, ".osrel") {
        Ok(osrel) => Ok(build_extra_cpio("os-release", osrel)),
        Err(_) => Ok(Vec::new()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `.cmdline` contents for `build_test_uki` (shared with the PCR 9 test).
    pub const TEST_UKI_CMDLINE: &str = r#"root=/dev/dm-0 rootwait ro dm-mod.create="root,,,ro,0 1 verity" -- systemd.log_target=journal"#;

    /// `.osrel` contents for `build_test_uki`; wrapped into the `/.extra/os-release`
    /// initrd cpio (shared with the PCR 9 test).
    pub const TEST_UKI_OSREL: &str = "ID=bottlerocket-test\nVERSION_ID=0.0\n";

    /// Build a mock shim PE with .sbatlevel and .vendor_cert sections.
    ///
    /// Uses COFF string table for section names longer than 8 bytes.
    /// Section name format for long names: "/" followed by decimal offset into string table.
    pub fn build_test_shim() -> Vec<u8> {
        let mut pe = vec![0u8; 0xa00];

        // DOS header
        pe[0..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes()); // PE header offset

        // PE signature + COFF header at 0x40
        pe[0x40..0x44].copy_from_slice(b"PE\0\0");
        pe[0x44..0x46].copy_from_slice(&0xaa64u16.to_le_bytes()); // Machine: ARM64
        pe[0x46..0x48].copy_from_slice(&3u16.to_le_bytes()); // NumberOfSections
        pe[0x4c..0x50].copy_from_slice(&0x800u32.to_le_bytes()); // PointerToSymbolTable (string table follows)
        pe[0x50..0x54].copy_from_slice(&0u32.to_le_bytes()); // NumberOfSymbols
        pe[0x54..0x56].copy_from_slice(&0xf0u16.to_le_bytes()); // SizeOfOptionalHeader
        pe[0x56..0x58].copy_from_slice(&0x22u16.to_le_bytes()); // Characteristics

        // Optional header (PE32+) at 0x58
        pe[0x58..0x5a].copy_from_slice(&0x20bu16.to_le_bytes()); // Magic: PE32+
        pe[0x5a] = 0x01; // MajorLinkerVersion
        pe[0x78..0x7c].copy_from_slice(&0x1000u32.to_le_bytes()); // SectionAlignment
        pe[0x7c..0x80].copy_from_slice(&0x200u32.to_le_bytes()); // FileAlignment
        pe[0x80..0x84].copy_from_slice(&1u32.to_le_bytes()); // MajorOperatingSystemVersion
        pe[0x88..0x8c].copy_from_slice(&1u32.to_le_bytes()); // MajorSubsystemVersion
        pe[0x90..0x94].copy_from_slice(&0x5000u32.to_le_bytes()); // SizeOfImage
        pe[0x94..0x98].copy_from_slice(&0x200u32.to_le_bytes()); // SizeOfHeaders
        pe[0x9c..0x9e].copy_from_slice(&10u16.to_le_bytes()); // Subsystem: EFI Application
        pe[0xc4..0xc8].copy_from_slice(&16u32.to_le_bytes()); // NumberOfRvaAndSizes

        // Section headers start at 0x148 (after optional header)
        // Section 1: .text (short name, fits in 8 bytes)
        pe[0x148..0x150].copy_from_slice(b".text\0\0\0");
        pe[0x150..0x154].copy_from_slice(&0x10u32.to_le_bytes()); // VirtualSize
        pe[0x154..0x158].copy_from_slice(&0x1000u32.to_le_bytes()); // VirtualAddress
        pe[0x158..0x15c].copy_from_slice(&0x200u32.to_le_bytes()); // SizeOfRawData
        pe[0x15c..0x160].copy_from_slice(&0x200u32.to_le_bytes()); // PointerToRawData
        pe[0x16c..0x170].copy_from_slice(&0x60000020u32.to_le_bytes()); // Characteristics

        // Section 2: .sbatlevel (long name -> "/4" = offset 4 in string table)
        pe[0x170..0x178].copy_from_slice(b"/4\0\0\0\0\0\0");
        pe[0x178..0x17c].copy_from_slice(&0x40u32.to_le_bytes()); // VirtualSize
        pe[0x17c..0x180].copy_from_slice(&0x2000u32.to_le_bytes()); // VirtualAddress
        pe[0x180..0x184].copy_from_slice(&0x200u32.to_le_bytes()); // SizeOfRawData
        pe[0x184..0x188].copy_from_slice(&0x400u32.to_le_bytes()); // PointerToRawData
        pe[0x194..0x198].copy_from_slice(&0x40000040u32.to_le_bytes()); // Characteristics

        // Section 3: .vendor_cert (long name -> "/15" = offset 15 in string table)
        pe[0x198..0x1a0].copy_from_slice(b"/15\0\0\0\0\0");
        pe[0x1a0..0x1a4].copy_from_slice(&0x30u32.to_le_bytes()); // VirtualSize
        pe[0x1a4..0x1a8].copy_from_slice(&0x3000u32.to_le_bytes()); // VirtualAddress
        pe[0x1a8..0x1ac].copy_from_slice(&0x200u32.to_le_bytes()); // SizeOfRawData
        pe[0x1ac..0x1b0].copy_from_slice(&0x600u32.to_le_bytes()); // PointerToRawData
        pe[0x1bc..0x1c0].copy_from_slice(&0x40000040u32.to_le_bytes()); // Characteristics

        // .text data at 0x200
        pe[0x200..0x210].fill(0xcc);

        // .sbatlevel data at 0x400: 12-byte header + SBAT string
        let sbat_str = b"sbat,1,2024010900\nshim,4\ngrub,3\n";
        pe[0x400..0x404].copy_from_slice(&0u32.to_le_bytes()); // unused
        pe[0x404..0x408].copy_from_slice(&12u32.to_le_bytes()); // offset to first string
        pe[0x408..0x40c].copy_from_slice(&0u32.to_le_bytes()); // unused
        pe[0x40c..0x40c + sbat_str.len()].copy_from_slice(sbat_str);

        // .vendor_cert data at 0x600: header + mock DER cert
        let mock_cert = [0x30, 0x82, 0x00, 0x10, 0x02, 0x01, 0x00]; // minimal ASN.1 SEQUENCE
        let cert_size = mock_cert.len() as u32;
        pe[0x600..0x604].copy_from_slice(&cert_size.to_le_bytes()); // vendor_authorized_size
        pe[0x604..0x608].copy_from_slice(&0u32.to_le_bytes()); // vendor_deauthorized_size
        pe[0x608..0x60c].copy_from_slice(&16u32.to_le_bytes()); // vendor_authorized_offset
        pe[0x60c..0x610].copy_from_slice(&0u32.to_le_bytes()); // vendor_deauthorized_offset
        pe[0x610..0x610 + mock_cert.len()].copy_from_slice(&mock_cert);

        // COFF string table at 0x800 (PointerToSymbolTable)
        // Format: 4-byte size followed by null-terminated strings
        // String table layout:
        //   offset 0-3: size (u32 LE)
        //   offset 4: ".sbatlevel\0" (11 bytes)
        //   offset 15: ".vendor_cert\0" (13 bytes)
        let strtab_size: u32 = 4 + 11 + 13; // size field + both strings
        pe[0x800..0x804].copy_from_slice(&strtab_size.to_le_bytes());
        pe[0x804..0x80f].copy_from_slice(b".sbatlevel\0");
        pe[0x80f..0x81c].copy_from_slice(b".vendor_cert\0");

        pe
    }

    /// Build a mock UKI PE (systemd-stub) with `.linux` (the `is_uki` marker),
    /// `.cmdline` and `.osrel` sections, enough to exercise PCR 9 prediction.
    pub fn build_test_uki() -> Vec<u8> {
        let mut pe = vec![0u8; 0xa00];

        // DOS header
        pe[0..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes()); // PE header offset

        // PE signature + COFF header at 0x40
        pe[0x40..0x44].copy_from_slice(b"PE\0\0");
        pe[0x44..0x46].copy_from_slice(&0x8664u16.to_le_bytes()); // Machine: x86_64
        pe[0x46..0x48].copy_from_slice(&4u16.to_le_bytes()); // NumberOfSections
        pe[0x4c..0x50].copy_from_slice(&0u32.to_le_bytes()); // PointerToSymbolTable
        pe[0x50..0x54].copy_from_slice(&0u32.to_le_bytes()); // NumberOfSymbols
        pe[0x54..0x56].copy_from_slice(&0xf0u16.to_le_bytes()); // SizeOfOptionalHeader
        pe[0x56..0x58].copy_from_slice(&0x22u16.to_le_bytes()); // Characteristics

        // Optional header (PE32+) at 0x58
        pe[0x58..0x5a].copy_from_slice(&0x20bu16.to_le_bytes()); // Magic: PE32+
        pe[0x5a] = 0x01; // MajorLinkerVersion
        pe[0x78..0x7c].copy_from_slice(&0x1000u32.to_le_bytes()); // SectionAlignment
        pe[0x7c..0x80].copy_from_slice(&0x200u32.to_le_bytes()); // FileAlignment
        pe[0x80..0x84].copy_from_slice(&1u32.to_le_bytes()); // MajorOperatingSystemVersion
        pe[0x88..0x8c].copy_from_slice(&1u32.to_le_bytes()); // MajorSubsystemVersion
        pe[0x90..0x94].copy_from_slice(&0x5000u32.to_le_bytes()); // SizeOfImage
        pe[0x94..0x98].copy_from_slice(&0x200u32.to_le_bytes()); // SizeOfHeaders
        pe[0x9c..0x9e].copy_from_slice(&10u16.to_le_bytes()); // Subsystem: EFI Application
        pe[0xc4..0xc8].copy_from_slice(&16u32.to_le_bytes()); // NumberOfRvaAndSizes

        // Section headers start at 0x148 (after optional header)
        // Section 1: .text
        pe[0x148..0x150].copy_from_slice(b".text\0\0\0");
        pe[0x150..0x154].copy_from_slice(&0x10u32.to_le_bytes()); // VirtualSize
        pe[0x154..0x158].copy_from_slice(&0x1000u32.to_le_bytes()); // VirtualAddress
        pe[0x158..0x15c].copy_from_slice(&0x200u32.to_le_bytes()); // SizeOfRawData
        pe[0x15c..0x160].copy_from_slice(&0x200u32.to_le_bytes()); // PointerToRawData
        pe[0x16c..0x170].copy_from_slice(&0x60000020u32.to_le_bytes()); // Characteristics

        // Section 2: .linux (short name, fits in 8 bytes)
        pe[0x170..0x178].copy_from_slice(b".linux\0\0");
        pe[0x178..0x17c].copy_from_slice(&0x40u32.to_le_bytes()); // VirtualSize
        pe[0x17c..0x180].copy_from_slice(&0x2000u32.to_le_bytes()); // VirtualAddress
        pe[0x180..0x184].copy_from_slice(&0x200u32.to_le_bytes()); // SizeOfRawData
        pe[0x184..0x188].copy_from_slice(&0x400u32.to_le_bytes()); // PointerToRawData
        pe[0x194..0x198].copy_from_slice(&0x40000040u32.to_le_bytes()); // Characteristics

        // Section 3: .cmdline (short name, exactly 8 bytes)
        pe[0x198..0x1a0].copy_from_slice(b".cmdline");
        pe[0x1a0..0x1a4].copy_from_slice(&0x200u32.to_le_bytes()); // VirtualSize
        pe[0x1a4..0x1a8].copy_from_slice(&0x3000u32.to_le_bytes()); // VirtualAddress
        pe[0x1a8..0x1ac].copy_from_slice(&0x200u32.to_le_bytes()); // SizeOfRawData
        pe[0x1ac..0x1b0].copy_from_slice(&0x600u32.to_le_bytes()); // PointerToRawData
        pe[0x1bc..0x1c0].copy_from_slice(&0x40000040u32.to_le_bytes()); // Characteristics

        // Section 4: .osrel. VirtualSize = exact length so the section (and its
        // cpio) has no NUL padding; systemd-stub measures the virtual size.
        let osrel = TEST_UKI_OSREL.as_bytes();
        pe[0x1c0..0x1c8].copy_from_slice(b".osrel\0\0");
        pe[0x1c8..0x1cc].copy_from_slice(&(osrel.len() as u32).to_le_bytes()); // VirtualSize
        pe[0x1cc..0x1d0].copy_from_slice(&0x4000u32.to_le_bytes()); // VirtualAddress
        pe[0x1d0..0x1d4].copy_from_slice(&0x200u32.to_le_bytes()); // SizeOfRawData
        pe[0x1d4..0x1d8].copy_from_slice(&0x800u32.to_le_bytes()); // PointerToRawData
        pe[0x1e4..0x1e8].copy_from_slice(&0x40000040u32.to_le_bytes()); // Characteristics

        // .text data at 0x200
        pe[0x200..0x210].fill(0xcc);
        // .linux data at 0x400 (mock embedded kernel bytes)
        pe[0x400..0x440].fill(0xab);
        // .cmdline data at 0x600 (NUL-padded kernel command line)
        let cmdline = TEST_UKI_CMDLINE.as_bytes();
        pe[0x600..0x600 + cmdline.len()].copy_from_slice(cmdline);
        // .osrel data at 0x800
        pe[0x800..0x800 + osrel.len()].copy_from_slice(osrel);

        pe
    }

    #[test]
    fn test_authenticode_hash() {
        let pe = build_test_shim();
        let hash = get_authenticode_hash(&pe).unwrap();
        assert_eq!(
            hex::encode(hash),
            "31afa5f057f8d697e026c2c21930bc01eec2e4b9ccf66d0006c9539d247aaa92"
        );
    }

    #[test]
    fn test_extract_vendor_cert() {
        let shim = build_test_shim();
        let cert = extract_vendor_cert(&shim).unwrap();
        assert_eq!(&cert[0..2], &[0x30, 0x82]); // ASN.1 SEQUENCE
        assert_eq!(cert.len(), 7);
    }

    #[test]
    fn test_extract_sbat_level() {
        let shim = build_test_shim();
        let sbat = extract_sbat_level(&shim).unwrap();
        let sbat_str = String::from_utf8_lossy(&sbat);
        assert!(sbat_str.starts_with("sbat,1,"));
        assert!(sbat_str.contains("shim,"));
    }

    #[test]
    fn test_sbat_no_null_terminator() {
        let mut shim = build_test_shim();
        shim[0x400..0x600].fill(0x41); // Fill .sbatlev section with 'A'

        // Re-add header
        shim[0x400..0x404].copy_from_slice(&0u32.to_le_bytes());
        shim[0x404..0x408].copy_from_slice(&12u32.to_le_bytes());
        shim[0x408..0x40c].copy_from_slice(&0u32.to_le_bytes());

        let err = extract_sbat_level(&shim).unwrap_err();
        assert!(err.to_string().contains("not null-terminated"));
    }

    #[test]
    fn test_sbat_section_too_small() {
        let mut shim = build_test_shim();
        shim[0x180..0x184].copy_from_slice(&10u32.to_le_bytes()); // SizeOfRawData = 10

        let err = extract_sbat_level(&shim).unwrap_err();
        assert!(err.to_string().contains("too small"));
    }

    #[test]
    fn test_vendor_cert_zero_size() {
        let mut shim = build_test_shim();
        shim[0x600..0x604].copy_from_slice(&0u32.to_le_bytes());

        let err = extract_vendor_cert(&shim).unwrap_err();
        assert!(err.to_string().contains("no vendor certificate"));
    }

    #[test]
    fn test_vendor_cert_extends_beyond_section() {
        let mut shim = build_test_shim();
        shim[0x600..0x604].copy_from_slice(&0xFFFFu32.to_le_bytes());

        let err = extract_vendor_cert(&shim).unwrap_err();
        assert!(err.to_string().contains("extends beyond"));
    }

    #[test]
    fn test_section_not_found() {
        let shim = build_test_shim();
        let err = get_section_data(&shim, ".nonexistent").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_is_uki_true() {
        let uki = build_test_uki();
        assert!(is_uki(&uki));
    }

    #[test]
    fn test_is_uki_false_for_shim() {
        // A shim has no `.linux` section.
        let shim = build_test_shim();
        assert!(!is_uki(&shim));
    }

    #[test]
    fn test_authenticode_hash_uki() {
        // The UKI test fixture must be Authenticode-hashable (used by PCR 4).
        let uki = build_test_uki();
        get_authenticode_hash(&uki).unwrap();
    }

    #[test]
    fn test_extract_uki_cmdline() {
        let uki = build_test_uki();
        let cmdline = extract_uki_cmdline(&uki).unwrap();
        // The extracted cmdline must equal the embedded string with the
        // trailing NUL padding stripped.
        assert_eq!(cmdline, TEST_UKI_CMDLINE);
    }

    #[test]
    fn test_uki_cmdline_load_options() {
        let uki = build_test_uki();
        let lo = uki_cmdline_load_options(&uki).unwrap();

        // The measured LoadOptions are the UTF-16LE encoding of the raw
        // `.cmdline` (no quote repair, no newline) plus a UTF-16 NUL terminator.
        let mut expected: Vec<u8> = TEST_UKI_CMDLINE
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        expected.extend_from_slice(&[0, 0]);
        assert_eq!(lo, expected);
        assert_eq!(lo.len(), TEST_UKI_CMDLINE.len() * 2 + 2);
        // The dm-mod.create quotes are preserved verbatim (NOT quote-repaired):
        // the `"` char (0x22) appears as the UTF-16LE unit 22 00.
        assert!(lo.windows(2).any(|w| w == [0x22, 0x00]));
    }

    #[test]
    fn test_build_uki_synthetic_initrd_osrel_cpio() {
        let uki = build_test_uki();
        let initrd = build_uki_synthetic_initrd(&uki).unwrap();

        // Golden SHA256 of the systemd-stub `/.extra/os-release` cpio built from
        // TEST_UKI_OSREL. Computed independently from the cpio `newc` format.
        let digest: [u8; 32] = Sha256::digest(&initrd).into();
        assert_eq!(
            hex::encode(digest),
            "ce5bfa54adc1aba0973b10e43e2e45ca7037d11fc27ca066fe7fa293bdd86d08"
        );

        // Structural sanity: a newc archive containing the .extra dir, the
        // os-release file and the trailer.
        assert!(initrd.starts_with(b"070701"));
        assert!(initrd.windows(6).any(|w| w == b".extra"));
        assert!(initrd.windows(10).any(|w| w == b"os-release"));
        assert!(initrd.windows(10).any(|w| w == b"TRAILER!!!"));
        assert!(initrd.len().is_multiple_of(4));
    }
}
