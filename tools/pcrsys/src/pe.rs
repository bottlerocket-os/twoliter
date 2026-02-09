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

    whatever!("section '{name}' not found")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

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
}
