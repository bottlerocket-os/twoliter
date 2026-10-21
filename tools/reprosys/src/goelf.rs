//! Go build IDs in ELF binaries.
//!
//! Go stores a build ID of four parts in each binary's `.note.go.buildid`:
//! `actionID(binary)/actionID(main.a)/contentID(main.a)/contentID(binary)`. The linker derives the
//! GNU build ID (`.note.gnu.build-id`) from the output, which includes the Go build ID.
//!
//! Go 1.25 to 1.27 randomize the action IDs of binaries using cgo packages built with LTO
//! (golang/go#81040, fixed in Go 1.28), and with them the GNU build ID. A content ID hashes the
//! output with the build ID excluded, so it stays the same when the code does: two builds of such
//! a binary differ only in their action IDs and GNU build ID. Nothing reads the build IDs at run
//! time.

use crate::error::Result;
use object::elf::{FileHeader32, FileHeader64};
use object::read::elf::{FileHeader, SectionHeader};
use object::{Endianness, FileKind};
use snafu::{ensure_whatever, FromString, OptionExt, ResultExt, Whatever};
use std::ops::Range;

const GO_BUILD_ID_SECTION: &[u8] = b".note.go.buildid";
const GO_NOTE_NAME: &[u8] = b"Go";
const GO_NOTE_TYPE: u32 = 4;
const GNU_BUILD_ID_SECTION: &[u8] = b".note.gnu.build-id";
const GNU_NOTE_NAME: &[u8] = b"GNU";
const GNU_NOTE_TYPE: u32 = 3;

/// A binary's Go build ID and the byte ranges of both build ID descriptors.
struct BuildIds<'a> {
    go: &'a str,
    masked: Vec<Range<usize>>,
}

fn build_ids(data: &[u8]) -> Result<Option<BuildIds<'_>>> {
    match FileKind::parse(data) {
        Ok(FileKind::Elf32) => build_ids_in::<FileHeader32<Endianness>>(data),
        Ok(FileKind::Elf64) => build_ids_in::<FileHeader64<Endianness>>(data),
        _ => Err(Whatever::without_source("not an ELF file".into())),
    }
}

fn build_ids_in<Elf: FileHeader<Endian = Endianness>>(data: &[u8]) -> Result<Option<BuildIds<'_>>> {
    let header = Elf::parse(data).whatever_context("invalid ELF header")?;
    let endian = header.endian().whatever_context("invalid ELF byte order")?;
    let sections = header
        .sections(endian, data)
        .whatever_context("invalid ELF section headers")?;
    // The descriptor of the first note in the section named `section`, if there is one, checked
    // to have the expected owner name and type.
    let note = |section: &[u8], name: &[u8], note_type: u32| -> Result<Option<Range<usize>>> {
        let section_name = String::from_utf8_lossy(section);
        let mut matching = sections
            .iter()
            .filter(|s| sections.section_name(endian, s) == Ok(section));
        let Some(header) = matching.next() else {
            return Ok(None);
        };
        ensure_whatever!(
            matching.next().is_none(),
            "more than one {section_name} section"
        );
        let desc = header
            .notes(endian, data)
            .with_whatever_context(|_| format!("invalid {section_name} section"))?
            .with_whatever_context(|| format!("{section_name} is not a note section"))?
            .next()
            .ok()
            .flatten()
            .filter(|n| n.name() == name && n.n_type(endian) == note_type)
            .with_whatever_context(|| format!("unexpected {section_name} note"))?
            .desc();
        // The descriptor borrows `data`, so its offset in it is where it is in the file.
        let start = desc.as_ptr() as usize - data.as_ptr() as usize;
        Ok(Some(start..start + desc.len()))
    };
    let Some(go) = note(GO_BUILD_ID_SECTION, GO_NOTE_NAME, GO_NOTE_TYPE)? else {
        return Ok(None);
    };
    let id = std::str::from_utf8(&data[go.clone()]).whatever_context("Go build ID is not UTF-8")?;
    let mut masked = vec![go];
    masked.extend(note(GNU_BUILD_ID_SECTION, GNU_NOTE_NAME, GNU_NOTE_TYPE)?);
    masked.sort_by_key(|r| r.start);
    ensure_whatever!(
        masked.windows(2).all(|w| w[0].end <= w[1].start),
        "build ID notes overlap"
    );
    Ok(Some(BuildIds { go: id, masked }))
}

/// Compare two ELF files that differ, when at least one is a Go binary.
///
/// Returns `None` when neither file has a Go build ID, so other rules apply. Otherwise returns
/// `Ok` with a description when the files differ only as golang/go#81040 explains: their Go build
/// IDs differ, and every byte outside the Go and GNU build ID descriptors is identical. Returns
/// `Err` with the reason otherwise.
pub fn compare_go_binaries(expected: &[u8], actual: &[u8]) -> Option<Result<String>> {
    let fail = |reason: String| Some(Err(Whatever::without_source(reason)));
    let (e, a) = match (build_ids(expected), build_ids(actual)) {
        (Ok(Some(e)), Ok(Some(a))) => (e, a),
        (Err(err), Ok(Some(_))) => return fail(format!("rebuilt binary: {err}")),
        (Ok(Some(_)), Err(err)) => return fail(format!("published binary: {err}")),
        (Ok(None), Ok(Some(_))) => {
            return fail("only the published binary has a Go build ID".into())
        }
        (Ok(Some(_)), Ok(None)) => return fail("only the rebuilt binary has a Go build ID".into()),
        _ => return None,
    };
    Some(describe(expected, actual, &e, &a))
}

fn describe(expected: &[u8], actual: &[u8], e: &BuildIds, a: &BuildIds) -> Result<String> {
    ensure_whatever!(
        e.go != a.go,
        "Go build IDs are identical ({}), so they do not explain the difference",
        e.go
    );
    let (Some(e_content), Some(a_content)) = (content_ids(e.go), content_ids(a.go)) else {
        snafu::whatever!(
            "Go build IDs are not four /-separated parts ({} vs {})",
            e.go,
            a.go
        );
    };
    ensure_whatever!(
        e_content == a_content,
        "Go content IDs differ ({} vs {}), so the code differs",
        e.go,
        a.go
    );
    ensure_whatever!(
        expected.len() == actual.len() && e.masked == a.masked,
        "build ID notes differ in layout ({} vs {} bytes)",
        expected.len(),
        actual.len()
    );
    let mut start = 0;
    for range in e.masked.iter().chain([&(expected.len()..expected.len())]) {
        let (e_span, a_span) = (&expected[start..range.start], &actual[start..range.start]);
        if let Some(i) = e_span.iter().zip(a_span).position(|(x, y)| x != y) {
            snafu::whatever!(
                "differs outside the build IDs, first at offset {:#x}",
                start + i
            );
        }
        start = range.end;
    }
    Ok(format!(
        "differs only in the Go action IDs ({} vs {}) and the GNU build ID (golang/go#81040)",
        e.go, a.go
    ))
}

/// The content IDs of a Go build ID, its last two parts, if it has four.
fn content_ids(id: &str) -> Option<[&str; 2]> {
    match id.split('/').collect::<Vec<_>>().as_slice() {
        [_, _, main, binary] => Some([*main, *binary]),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use object::elf::SHT_NOTE;

    /// Write the low `width` bytes of `value` at `at`, little-endian.
    fn put(data: &mut [u8], at: usize, width: usize, value: u64) {
        data[at..at + width].copy_from_slice(&value.to_le_bytes()[..width]);
    }

    /// Build a minimal ELF64 little-endian file with a Go build ID note, a GNU build ID note
    /// and a `.text` section holding `text`.
    pub(crate) fn go_elf(build_id: &str, gnu_id: &[u8; 20], text: &[u8]) -> Vec<u8> {
        const HEADER_SIZE: usize = 64;
        const SHENTSIZE: usize = 64;
        const WORD: usize = 8;
        const SHOFF_FIELD: usize = 0x28;
        const SHENTSIZE_FIELD: usize = 0x3a;
        const OFFSET_FIELD: usize = 0x18;
        const SIZE_FIELD: usize = 0x20;
        let note = |name: &[u8], ty: u32, desc: &[u8]| {
            let mut n = vec![0u8; 12];
            put(&mut n, 0, 4, name.len() as u64 + 1);
            put(&mut n, 4, 4, desc.len() as u64);
            put(&mut n, 8, 4, ty.into());
            n.extend(name);
            n.push(0);
            n.resize(n.len().next_multiple_of(4), 0);
            n.extend(desc);
            n.resize(n.len().next_multiple_of(4), 0);
            n
        };
        let strtab = b"\0.note.gnu.build-id\0.note.go.buildid\0.text\0.shstrtab\0".to_vec();
        let name_of = |s: &[u8]| strtab.windows(s.len()).position(|w| w == s).unwrap() as u64;
        let contents: Vec<(u64, u32, Vec<u8>)> = vec![
            (
                name_of(b".note.gnu.build-id"),
                SHT_NOTE,
                note(GNU_NOTE_NAME, GNU_NOTE_TYPE, gnu_id),
            ),
            (
                name_of(b".note.go.buildid"),
                SHT_NOTE,
                note(GO_NOTE_NAME, GO_NOTE_TYPE, build_id.as_bytes()),
            ),
            (name_of(b".text"), 1, text.to_vec()),
            (name_of(b".shstrtab"), 3, strtab.clone()),
        ];
        let mut data = vec![0u8; HEADER_SIZE];
        data[..4].copy_from_slice(b"\x7fELF");
        data[4] = 2;
        data[5] = 1;
        data[6] = 1;
        let mut headers = vec![vec![0u8; SHENTSIZE]];
        for (name, ty, body) in &contents {
            let offset = data.len() as u64;
            data.extend(body);
            data.resize(data.len().next_multiple_of(4), 0);
            let mut h = vec![0u8; SHENTSIZE];
            put(&mut h, 0, 4, *name);
            put(&mut h, 4, 4, (*ty).into());
            put(&mut h, OFFSET_FIELD, WORD, offset);
            put(&mut h, SIZE_FIELD, WORD, body.len() as u64);
            headers.push(h);
        }
        data.resize(data.len().next_multiple_of(8), 0);
        let shoff = data.len() as u64;
        put(&mut data, SHOFF_FIELD, WORD, shoff);
        put(&mut data, SHENTSIZE_FIELD, 2, SHENTSIZE as u64);
        put(&mut data, SHENTSIZE_FIELD + 2, 2, headers.len() as u64);
        put(&mut data, SHENTSIZE_FIELD + 4, 2, headers.len() as u64 - 1);
        data.extend(headers.concat());
        data
    }

    pub(crate) const REBUILT: &str =
        "OekQOI0MOb1L1q0FhHFu/hNEH2tEo8qcxP8hA3l_K/Q_z97QRB_xGDtgJGHBTy/d-BksYDz9tl4qL7vlNeh";
    pub(crate) const PUBLISHED: &str =
        "xDItdwtzBGChhBqMlNpC/z0GQ1EV_YXxQ5RJ6sZEy/Q_z97QRB_xGDtgJGHBTy/d-BksYDz9tl4qL7vlNeh";

    #[test]
    fn accepts_only_build_id_differences() {
        let e = go_elf(REBUILT, &[1; 20], b"code");
        let a = go_elf(PUBLISHED, &[2; 20], b"code");
        let detail = compare_go_binaries(&e, &a).unwrap().unwrap();
        assert!(detail.contains("golang/go#81040"), "{detail}");
        let other_code = go_elf(PUBLISHED, &[2; 20], b"c0de");
        let err = compare_go_binaries(&e, &other_code).unwrap().unwrap_err();
        assert!(err.to_string().contains("outside the build IDs"), "{err}");
    }

    #[test]
    fn rejects_other_differences() {
        let e = go_elf(REBUILT, &[1; 20], b"code");

        let err = compare_go_binaries(&e, &go_elf(REBUILT, &[2; 20], b"code"))
            .unwrap()
            .unwrap_err();
        assert!(err.to_string().contains("identical"), "{err}");

        for other in [
            "xDItdwtzBGChhBqMlNpC/z0GQ1EV_YXxQ5RJ6sZEy/Q_z97QRB_xGDtgJGHBTy/other",
            "xDItdwtzBGChhBqMlNpC/z0GQ1EV_YXxQ5RJ6sZEy/other/d-BksYDz9tl4qL7vlNeh",
        ] {
            let a = go_elf(other, &[2; 20], b"code");
            let err = compare_go_binaries(&e, &a).unwrap().unwrap_err();
            assert!(err.to_string().contains("content IDs differ"), "{err}");
        }
        let a = go_elf("xDItdwtzBGChhBqMlNpC/other", &[2; 20], b"code");
        let err = compare_go_binaries(&e, &a).unwrap().unwrap_err();
        assert!(err.to_string().contains("four"), "{err}");
    }

    #[test]
    fn rejects_overlapping_build_ids() {
        let gnu_note = [
            &4u32.to_le_bytes()[..],
            &20u32.to_le_bytes(),
            &GNU_NOTE_TYPE.to_le_bytes(),
            b"GNU\0",
            &[b'x'; 20],
        ]
        .concat();
        // Four bytes long, so that the embedded note is aligned.
        let prefix = "aaaa/bbbb/ccccc/";
        let id = format!("{prefix}{}", std::str::from_utf8(&gnu_note).unwrap());
        let mut data = go_elf(&id, &[1; 20], b"code");
        let shoff = u64::from_le_bytes(data[0x28..0x30].try_into().unwrap()) as usize;
        let go_header = shoff + 2 * 64;
        let go_offset = u64::from_le_bytes(data[go_header + 0x18..][..8].try_into().unwrap());
        let embedded = go_offset + 12 + 4 + prefix.len() as u64;
        let gnu_header = shoff + 64;
        data[gnu_header + 0x18..][..8].copy_from_slice(&embedded.to_le_bytes());
        data[gnu_header + 0x20..][..8].copy_from_slice(&(gnu_note.len() as u64).to_le_bytes());

        let valid = go_elf(REBUILT, &[1; 20], b"code");
        let err = compare_go_binaries(&valid, &data).unwrap().unwrap_err();
        assert!(err.to_string().contains("build ID notes overlap"), "{err}");
    }

    #[test]
    fn ignores_files_without_go_build_ids() {
        assert!(compare_go_binaries(b"\x7fELFjunk", b"\x7fELFjunk2").is_none());
        let e = go_elf(REBUILT, &[1; 20], b"code");
        let err = compare_go_binaries(&e, b"\x7fELFjunk")
            .unwrap()
            .unwrap_err();
        assert!(err.to_string().contains("published binary"), "{err}");
    }
}
