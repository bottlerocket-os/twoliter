//! PE file parsing and Authenticode hash calculation.
//!
//! This matches the behavior of `pesign --hash`.

use crate::error::Result;
use authenticode::authenticode_digest;
use object::read::pe::PeFile;
use object::{pe, Architecture, Object, ObjectSection};
use sha2::{Digest, Sha256};
use snafu::{ensure_whatever, whatever, OptionExt, ResultExt};
use std::borrow::Cow;
use std::collections::BTreeMap;

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

    whatever!("section '{name}' not found");
}

// Bottlerocket's sections in systemd v257.13 enumeration order (uki.h/uki.c).
// Together with UNSUPPORTED_SECTIONS this covers that stub's full enumeration.
// Other PE sections (for example .text) enter PCR4, but not the stub's PCR11.
pub(crate) const UKI_SECTIONS: &[&str] = &[".linux", ".osrel", ".cmdline", ".uname", ".sbat"];
// rpm2img does not emit these. Supporting them requires additional measurements
// or profile/hardware selection; reject even empty section markers.
const UNSUPPORTED_SECTIONS: &[&str] = &[
    ".initrd", ".ucode", ".splash", ".dtb", ".pcrsig", ".pcrpkey", ".profile", ".dtbauto", ".hwids",
];

pub(crate) struct UkiImage<'a> {
    pub(crate) image: &'a [u8],
    sections: BTreeMap<&'a str, &'a [u8]>,
}

impl<'a> UkiImage<'a> {
    /// `None` means a valid PE without UKI markers. Malformed or partially
    /// populated UKIs must never fall through to GRUB discovery.
    pub(crate) fn parse(image: &'a [u8], machine: u16) -> Result<Option<Self>> {
        let pe = PeFile::<pe::ImageNtHeaders64>::parse(image)
            .whatever_context("failed to parse firmware fallback PE32+ image")?;
        ensure_whatever!(
            matches!(
                (machine, pe.architecture()),
                (0x8664, Architecture::X86_64) | (0xaa64, Architecture::Aarch64)
            ),
            "fallback filename and PE architecture disagree"
        );
        let mut sections = BTreeMap::new();
        let mut uki_marker = false;
        let mut ranges = Vec::new();
        for section in pe.sections() {
            let name = section.name().whatever_context("invalid PE section name")?;
            let header = section.pe_section();
            if section.size() > 0 {
                let start = u64::from(header.virtual_address.get(object::LittleEndian));
                let end = start + section.size();
                ensure_whatever!(
                    start > 0
                        && end
                            <= u64::from(
                                pe.nt_headers()
                                    .optional_header
                                    .size_of_image
                                    .get(object::LittleEndian)
                            ),
                    "UKI section '{name}' is outside the loaded image"
                );
                ensure_whatever!(
                    ranges.iter().all(|(a, b)| end <= *a || start >= *b),
                    "overlapping UKI sections"
                );
                ranges.push((start, end));
            }
            if UKI_SECTIONS.contains(&name) || UNSUPPORTED_SECTIONS.contains(&name) {
                ensure_whatever!(
                    header.name[0] != b'/',
                    "indirect UKI section names are unsupported"
                );
                ensure_whatever!(
                    !sections.contains_key(name),
                    "duplicate UKI section '{name}'"
                );
                let data = section
                    .data()
                    .whatever_context("invalid UKI section range")?;
                let size = usize::try_from(section.size())
                    .whatever_context("UKI section size overflow")?;
                ensure_whatever!(
                    size <= data.len(),
                    "unsupported zero-filled UKI section '{name}'"
                );
                sections.insert(name, &data[..size]);
                uki_marker |= matches!(name, ".linux" | ".osrel" | ".cmdline" | ".profile");
            }
        }
        if !uki_marker {
            return Ok(None);
        }
        for required in [".linux", ".osrel", ".cmdline", ".uname"] {
            ensure_whatever!(
                sections.get(required).is_some_and(|data| !data.is_empty()),
                "UKI missing required section '{required}'"
            );
        }
        for unsupported in UNSUPPORTED_SECTIONS {
            ensure_whatever!(
                !sections.contains_key(*unsupported),
                "unsupported UKI section '{unsupported}'"
            );
        }
        let kernel = PeFile::<pe::ImageNtHeaders64>::parse(sections[".linux"])
            .whatever_context("embedded kernel is not PE32+")?;
        ensure_whatever!(
            kernel.architecture() == pe.architecture(),
            "UKI and kernel architecture disagree"
        );
        let uki = Self { image, sections };
        uki.cmdline()?;
        Ok(Some(uki))
    }

    pub(crate) fn section(&self, name: &str) -> &[u8] {
        self.sections.get(name).copied().unwrap_or_default()
    }

    pub(crate) fn cmdline(&self) -> Result<&str> {
        let data = self.section(".cmdline");
        let end = data.iter().position(|b| *b == 0).unwrap_or(data.len());
        ensure_whatever!(
            data[end..].iter().all(|b| *b == 0),
            "UKI command line has data after NUL"
        );
        let cmdline =
            std::str::from_utf8(&data[..end]).whatever_context("UKI command line is not UTF-8")?;
        ensure_whatever!(
            !cmdline.is_empty() && cmdline.len() < 2048,
            "unsupported UKI command-line length"
        );
        // These disable or add inputs to the kernel's EFI initrd load path.
        ensure_whatever!(
            !cmdline
                .replace('"', "")
                .split_ascii_whitespace()
                .any(|arg| arg == "noinitrd"
                    || arg.starts_with("initrd=")
                    || arg.starts_with("efi=")),
            "unsupported EFI/initrd command-line override"
        );
        Ok(cmdline)
    }
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
    // Minimal PE32+ input, following pe.rs::tests::build_test_shim. Expected
    // measurements below remain fixed, independent of the predictor.
    fn build_test_pe(machine: u16, sections: &[(&str, &[u8])]) -> Vec<u8> {
        let mut image = vec![0u8; 1024];
        image[..2].copy_from_slice(b"MZ");
        image[0x40..0x44].copy_from_slice(b"PE\0\0");
        for (offset, value) in [
            (0x44, machine),
            (0x46, sections.len() as u16),
            (0x54, 0xf0),
            (0x56, 0x22),
            (0x58, 0x20b),
            (0x9c, 10),
        ] {
            image[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        image[0x70..0x78].copy_from_slice(&0x400000u64.to_le_bytes());
        for (offset, value) in [
            (0x3c, 0x40u32),
            (0x78, 4096),
            (0x7c, 512),
            (0x90, (sections.len() as u32 + 1) * 4096),
            (0x94, 1024),
            (0xc4, 16),
        ] {
            image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (index, (name, data)) in sections.iter().enumerate() {
            let header = 0x148 + 40 * index;
            let offset = image.len();
            let raw_size = data.len().next_multiple_of(512);
            image[header..header + name.len()].copy_from_slice(name.as_bytes());
            for (field, value) in [
                (8, data.len() as u32),
                (12, (index as u32 + 1) * 4096),
                (16, raw_size as u32),
                (20, offset as u32),
                (36, 0x40000040),
            ] {
                image[header + field..header + field + 4].copy_from_slice(&value.to_le_bytes());
            }
            image.extend_from_slice(data);
            image.resize(offset + raw_size, 0);
        }
        image
    }

    pub(crate) fn build_test_uki(machine: u16) -> Vec<u8> {
        let kernel = build_test_pe(machine, &[(".text", b"fixture kernel")]);
        // File order deliberately differs from systemd's measurement order.
        build_test_pe(
            machine,
            &[
                (
                    ".cmdline",
                    b"console=ttyS0 dm-mod.create=\"root test\" -- systemd.unit=fipscheck.target\0",
                ),
                (".uname", b"6.18.48\0"),
                (".osrel", b"ID=bottlerocket\n"),
                (".linux", &kernel),
                (".sbat", b"sbat,1\n"),
            ],
        )
    }

    pub(crate) fn efi_vars() -> crate::efi::EfiVars {
        use crate::efi::{
            generate_x509_esl, EfiVar, EFI_GLOBAL_VARIABLE_GUID, EFI_IMAGE_SECURITY_DATABASE_GUID,
        };
        crate::efi::EfiVars {
            variables: ["PK", "KEK", "db", "dbx"]
                .into_iter()
                .map(|name| {
                    let guid = if matches!(name, "PK" | "KEK") {
                        EFI_GLOBAL_VARIABLE_GUID
                    } else {
                        EFI_IMAGE_SECURITY_DATABASE_GUID
                    };
                    let data = generate_x509_esl(b"fixture certificate").unwrap();
                    EfiVar::new(name, guid.to_string(), hex::encode(data))
                })
                .collect(),
        }
    }

    #[test]
    fn bad_architecture_and_partial_uki_are_errors() {
        let image = build_test_uki(0x8664);
        assert!(UkiImage::parse(&image, 0xaa64).is_err());
        for len in [0, 64, 1024, image.len() - 512] {
            assert!(UkiImage::parse(&image[..len], 0x8664).is_err());
        }
        let mut image = build_test_uki(0x8664);
        image[0x148..0x150].copy_from_slice(b".missing");
        assert!(UkiImage::parse(&image, 0x8664).is_err());
    }

    #[test]
    fn duplicate_and_unsupported_sections_are_rejected() {
        for name in [
            b".osrel\0\0",
            b".profile",
            b".dtbauto",
            b".pcrsig\0",
            b".initrd\0",
            b".ucode\0\0",
            b".splash\0",
        ] {
            let mut image = build_test_uki(0x8664);
            // Replace optional .sbat, keeping every required section intact.
            image[0x1e8..0x1f0].copy_from_slice(name);
            assert!(UkiImage::parse(&image, 0x8664).is_err());
        }
    }

    #[test]
    fn unrelated_sections_cannot_overwrite_loaded_payload_bytes() {
        let mut image = build_test_uki(0x8664);
        // Rename .sbat to an unmeasured section and overlap it with .cmdline.
        image[0x1e8..0x1f0].copy_from_slice(b".other\0\0");
        image[0x1f4..0x1f8].copy_from_slice(&4096u32.to_le_bytes());
        let error = UkiImage::parse(&image, 0x8664).err().unwrap();
        assert!(error.to_string().contains("overlapping"));
    }
}
