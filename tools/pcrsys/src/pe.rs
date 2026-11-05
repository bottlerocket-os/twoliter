//! PE file parsing and Authenticode hash calculation.
//!
//! This matches the behavior of `pesign --hash`.

use crate::error::Result;
use crate::predict::PcrDigests;
use authenticode::authenticode_digest;
use bootconfig::UkiCmdline;
use object::read::pe::PeFile64;
use object::{Object, ObjectSection};
use sha2::digest::{Output, Update};
use sha2::{Digest, Sha256, Sha384};
use snafu::{ensure_whatever, whatever, OptionExt, ResultExt};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Deref;

/// Sections that mark a UKI, all of which a UKI must carry. `.sbat` is deliberately not one,
/// since shim also has it.
const UKI_MARKERS: [&str; 3] = [".linux", ".osrel", ".cmdline"];

/// A validated PE32+ image, reduced to what measurements need.
#[derive(Debug, Clone)]
pub struct PeImage {
    arch: Arch,
    authenticode: PcrDigests,
    sections: BTreeMap<String, Vec<u8>>,
}

impl PeImage {
    pub fn parse(data: &[u8]) -> Result<Self> {
        // pesign hashes the file padded to an 8-byte boundary.
        let data: Cow<[u8]> = if data.len().is_multiple_of(8) {
            data.into()
        } else {
            let mut padded = data.to_vec();
            padded.resize(data.len().next_multiple_of(8), 0);
            padded.into()
        };
        let pe = PeFile64::parse(&*data)
            .ok()
            .whatever_context("failed to parse PE32+ file")?;
        let arch = match pe.architecture() {
            object::Architecture::X86_64 => Arch::X86_64,
            object::Architecture::Aarch64 => Arch::Aarch64,
            other => {
                whatever!("unsupported architecture {other:?}");
            }
        };
        let mut sections = BTreeMap::new();
        for section in pe.sections() {
            let name = section
                .name()
                .whatever_context("failed to read PE section name")?;
            let data = section
                .data()
                .with_whatever_context(|_| format!("failed to read section '{name}'"))?;
            // The first section of a name wins, as with `section_by_name`.
            sections
                .entry(name.to_string())
                .or_insert_with(|| data.to_vec());
        }
        Ok(Self {
            arch,
            authenticode: PcrDigests {
                sha256: authenticode_hash::<Sha256>(&pe)?.into(),
                sha384: authenticode_hash::<Sha384>(&pe)?.into(),
            },
            sections,
        })
    }

    pub fn arch(&self) -> Arch {
        self.arch
    }

    pub fn authenticode(&self) -> &PcrDigests {
        &self.authenticode
    }

    /// Initialized bytes of the named section.
    pub fn section(&self, name: &str) -> Option<&[u8]> {
        self.sections.get(name).map(Vec::as_slice)
    }

    fn required_section(&self, name: &str) -> Result<&[u8]> {
        self.section(name)
            .with_whatever_context(|| format!("section '{name}' not found"))
    }
}

/// The CPU architectures Bottlerocket boots on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

/// A UKI: a PE image carrying the kernel, os-release, and command line.
#[derive(Debug, Clone)]
pub struct Uki {
    pe: PeImage,
    cmdline: UkiCmdline,
}

impl Uki {
    /// Classify a firmware fallback executable, handing it back when it is not a UKI.
    ///
    /// Fails for a UKI without every marker section, since its measurements cannot be predicted.
    pub fn from_pe(pe: PeImage) -> Result<std::result::Result<Self, PeImage>> {
        let has = |name| pe.section(name).is_some_and(|data| !data.is_empty());
        if !UKI_MARKERS.into_iter().any(has) {
            return Ok(Err(pe));
        }
        for name in UKI_MARKERS {
            ensure_whatever!(has(name), "UKI section '{name}' is missing or empty");
        }

        let cmdline = UkiCmdline::decode(pe.required_section(".cmdline")?)
            .whatever_context("failed to parse UKI command line")?;
        Ok(Ok(Self { pe, cmdline }))
    }

    /// The kernel command line the UKI was built with, as systemd-stub passes it on.
    pub fn cmdline(&self) -> &UkiCmdline {
        &self.cmdline
    }

    /// The os-release systemd-stub passes the kernel in its generated initrd.
    pub fn osrel(&self) -> &[u8] {
        self.section(".osrel").expect("from_pe requires .osrel")
    }
}

impl Deref for Uki {
    type Target = PeImage;

    fn deref(&self) -> &PeImage {
        &self.pe
    }
}

/// Shim: a PE image carrying the vendor certificate and SBAT level it measures.
#[derive(Debug, Clone)]
pub struct Shim {
    pe: PeImage,
    vendor_cert: Vec<u8>,
    sbat_level: Vec<u8>,
}

impl Shim {
    pub fn from_pe(pe: PeImage) -> Result<Self> {
        let vendor_cert = vendor_cert(pe.required_section(".vendor_cert")?)?.to_vec();
        let sbat_level = sbat_level(pe.required_section(".sbatlevel")?)?.to_vec();
        Ok(Self {
            pe,
            vendor_cert,
            sbat_level,
        })
    }

    /// The DER-encoded X.509 certificate shim trusts, which it measures into MokListRT.
    pub fn vendor_cert(&self) -> &[u8] {
        &self.vendor_cert
    }

    /// The automatic SBAT level, which shim measures into PCR 7.
    pub fn sbat_level(&self) -> &[u8] {
        &self.sbat_level
    }
}

impl Deref for Shim {
    type Target = PeImage;

    fn deref(&self) -> &PeImage {
        &self.pe
    }
}

/// The Authenticode digest of `pe` under the hash `D`.
fn authenticode_hash<D: Digest + Update>(pe: &PeFile64) -> Result<Output<D>> {
    let mut hasher = D::new();
    ensure_whatever!(
        authenticode_digest(pe, &mut hasher).is_ok(),
        "authenticode digest failed"
    );
    Ok(hasher.finalize())
}

/// The vendor certificate in shim's `.vendor_cert` section.
///
/// The section contains a header with certificate offsets and sizes:
/// - bytes 0-3: vendor_authorized_size (u32 LE)
/// - bytes 4-7: vendor_deauthorized_size (u32 LE)
/// - bytes 8-11: vendor_authorized_offset (u32 LE)
/// - bytes 12-15: vendor_deauthorized_offset (u32 LE)
/// - followed by certificate data
fn vendor_cert(section_data: &[u8]) -> Result<&[u8]> {
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

    Ok(&section_data[authorized_offset..authorized_offset + authorized_size])
}

/// The first (automatic) SBAT level string in shim's `.sbatlevel` section.
///
/// The section contains a 12-byte header followed by null-terminated SBAT strings:
/// - bytes 0-3: unused (u32 LE)
/// - bytes 4-7: offset to first string (u32 LE)
/// - bytes 8-11: unused (u32 LE)
/// - bytes 12+: null-terminated SBAT strings
fn sbat_level(section_data: &[u8]) -> Result<&[u8]> {
    const HEADER_SIZE: usize = 12;

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

    Ok(&section_data[HEADER_SIZE..end])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use test_case::test_case;

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

    /// Minimal PE32+ image with the given sections.
    pub(crate) fn build_test_pe(machine: u16, sections: &[(&str, &[u8])]) -> Vec<u8> {
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
        build_test_pe(
            machine,
            &[
                (".cmdline", b"quiet\0"),
                (".uname", b"6.18.48\0"),
                (".osrel", b"ID=bottlerocket\n"),
                (".linux", &kernel),
                (".sbat", b"sbat,1\n"),
            ],
        )
    }

    #[test]
    fn the_architecture_comes_from_the_machine_type() {
        let pe = |machine| build_test_pe(machine, &[(".text", b"kernel")]);
        let arch = |machine| PeImage::parse(&pe(machine)).map(|pe| pe.arch());
        assert_eq!(arch(0x8664).unwrap(), Arch::X86_64);
        assert_eq!(arch(0xaa64).unwrap(), Arch::Aarch64);
        let err = arch(0x5064).unwrap_err();
        assert!(
            err.to_string().contains("unsupported architecture"),
            "{err}"
        );
    }

    #[test]
    fn only_images_with_uki_sections_are_ukis() {
        let uki = PeImage::parse(&build_test_uki(0x8664)).unwrap();
        let uki = Uki::from_pe(uki).unwrap().unwrap();
        assert_eq!(uki.cmdline().to_string(), "quiet");
        assert_eq!(uki.osrel(), b"ID=bottlerocket\n");
        let shim = PeImage::parse(&build_test_shim()).unwrap();
        assert!(Uki::from_pe(shim).unwrap().is_err());
    }

    #[test_case(".linux", None, "'.linux' is missing or empty" ; "without_linux")]
    #[test_case(".osrel", None, "'.osrel' is missing or empty" ; "without_osrel")]
    #[test_case(".cmdline", None, "'.cmdline' is missing or empty" ; "without_cmdline")]
    #[test_case(".osrel", Some(b""), "'.osrel' is missing or empty" ; "with_an_empty_osrel")]
    #[test_case(".cmdline", Some(b"quiet\0extra"), "failed to parse UKI command line" ; "with_an_undecodable_cmdline")]
    #[test_case(".cmdline", Some("quiet é".as_bytes()), "failed to parse UKI command line" ; "with_a_non_ascii_cmdline")]
    #[test_case(".cmdline", Some(b"quiet\tro"), "failed to parse UKI command line" ; "with_a_tab_in_the_cmdline")]
    fn a_uki_the_stub_would_measure_differently_is_rejected(
        name: &str,
        replacement: Option<&[u8]>,
        expected: &str,
    ) {
        let kernel = build_test_pe(0x8664, &[(".text", b"fixture kernel")]);
        let sections: Vec<(&str, &[u8])> = [
            (".cmdline", b"quiet\0".as_slice()),
            (".osrel", b"ID=bottlerocket\n"),
            (".linux", &kernel),
        ]
        .into_iter()
        .filter_map(|(section, data)| {
            if section == name {
                replacement.map(|data| (section, data))
            } else {
                Some((section, data))
            }
        })
        .collect();
        let pe = PeImage::parse(&build_test_pe(0x8664, &sections)).unwrap();
        let err = Uki::from_pe(pe).unwrap_err();
        assert!(err.to_string().contains(expected), "{err}");
    }

    #[test]
    fn test_authenticode_hash() {
        let pe = build_test_shim();
        let hash = PeImage::parse(&pe).unwrap().authenticode;
        assert_eq!(
            hex::encode(hash.sha256),
            "31afa5f057f8d697e026c2c21930bc01eec2e4b9ccf66d0006c9539d247aaa92"
        );
    }

    #[test]
    fn the_hash_covers_the_file_zero_padded_to_eight_bytes() {
        let mut image = build_test_shim();
        image.extend_from_slice(b"abc");
        let mut padded = image.clone();
        padded.resize(image.len().next_multiple_of(8), 0);
        assert_eq!(
            PeImage::parse(&image).unwrap().authenticode,
            PeImage::parse(&padded).unwrap().authenticode
        );
    }

    #[test]
    fn a_shim_carries_its_vendor_cert_and_sbat_level() {
        let shim = Shim::from_pe(PeImage::parse(&build_test_shim()).unwrap()).unwrap();
        assert_eq!(&shim.vendor_cert()[0..2], &[0x30, 0x82]); // ASN.1 SEQUENCE
        assert_eq!(shim.vendor_cert().len(), 7);
        let sbat = String::from_utf8_lossy(shim.sbat_level());
        assert!(sbat.starts_with("sbat,1,"));
        assert!(sbat.contains("shim,"));
    }

    fn shim_error(image: &[u8]) -> String {
        let pe = PeImage::parse(image).unwrap();
        Shim::from_pe(pe).unwrap_err().to_string()
    }

    #[test]
    fn a_shim_without_a_vendor_cert_is_rejected() {
        let image = build_test_pe(0xaa64, &[(".text", b"shim")]);
        assert!(shim_error(&image).contains("'.vendor_cert' not found"));
    }

    #[test]
    fn test_sbat_no_null_terminator() {
        let mut shim = build_test_shim();
        shim[0x400..0x600].fill(0x41); // Fill .sbatlev section with 'A'

        // Re-add header
        shim[0x400..0x404].copy_from_slice(&0u32.to_le_bytes());
        shim[0x404..0x408].copy_from_slice(&12u32.to_le_bytes());
        shim[0x408..0x40c].copy_from_slice(&0u32.to_le_bytes());

        assert!(shim_error(&shim).contains("not null-terminated"));
    }

    #[test]
    fn test_sbat_section_too_small() {
        let mut shim = build_test_shim();
        shim[0x180..0x184].copy_from_slice(&10u32.to_le_bytes()); // SizeOfRawData = 10

        assert!(shim_error(&shim).contains("too small"));
    }

    #[test]
    fn test_vendor_cert_zero_size() {
        let mut shim = build_test_shim();
        shim[0x600..0x604].copy_from_slice(&0u32.to_le_bytes());

        assert!(shim_error(&shim).contains("no vendor certificate"));
    }

    #[test]
    fn test_vendor_cert_extends_beyond_section() {
        let mut shim = build_test_shim();
        shim[0x600..0x604].copy_from_slice(&0xFFFFu32.to_le_bytes());

        assert!(shim_error(&shim).contains("extends beyond"));
    }

    #[test]
    fn test_section_not_found() {
        let shim = build_test_shim();
        let err = PeImage::parse(&shim)
            .unwrap()
            .required_section(".nonexistent")
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
