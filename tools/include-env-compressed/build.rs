use std::path::PathBuf;

fn main() {
    // Set an environment variable to be used during integration tests
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("build.rs");
    println!("cargo:rustc-env=MY_BUILD_SCRIPT={}", manifest.display())
}
