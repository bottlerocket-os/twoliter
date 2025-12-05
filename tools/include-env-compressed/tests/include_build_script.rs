use include_env_compressed::ArchiveKind;

const BUILD_SCRIPT_CONTENTS: &str = include_str!("../build.rs");

#[test]
fn test_include_build_script() {
    let build_script = include_env_compressed::include_archive_from_env!("MY_BUILD_SCRIPT");
    let included_build_script = std::io::read_to_string(build_script.reader()).unwrap();
    assert_eq!(included_build_script.trim(), BUILD_SCRIPT_CONTENTS.trim());

    // The macro uses cfg(debug_assertions) to decide compression
    #[cfg(debug_assertions)]
    assert_eq!(build_script.kind(), ArchiveKind::Uncompressed);
    #[cfg(not(debug_assertions))]
    assert_eq!(build_script.kind(), ArchiveKind::Zstd);
}

#[test]
fn test_include_compressed() {
    let build_script =
        include_env_compressed_macro::include_zstd_archive_from_env!("MY_BUILD_SCRIPT");
    let included_build_script = std::io::read_to_string(build_script.reader()).unwrap();
    assert_eq!(included_build_script.trim(), BUILD_SCRIPT_CONTENTS.trim());
    assert_eq!(build_script.kind(), ArchiveKind::Zstd);
}
