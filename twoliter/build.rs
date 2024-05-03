/*!

Prepare and package embedded tools in a tarball to be included with Twoliter.

!*/

// The performance cost of this is infinitesimal, and we get a better panic stack with `expect`.
#![allow(clippy::expect_fun_call)]

use bytes::BufMut;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{env, fs};

const DATA_INPUT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/embedded");

fn main() {
    let paths = Paths::new();
    println!("cargo:rerun-if-changed={}", paths.data_input_dir.display());

    let _ = fs::remove_dir_all(&paths.prep_dir);
    fs::create_dir_all(&paths.prep_dir).expect(&format!(
        "Unable to create directory '{}'",
        paths.prep_dir.display()
    ));

    paths.copy_file("Dockerfile");
    paths.copy_file("Dockerfile.dockerignore");
    paths.copy_file("Makefile.toml");
    paths.copy_file("docker-go");
    paths.copy_file("partyplanner");
    paths.copy_file("rpm2img");
    paths.copy_file("rpm2kit");
    paths.copy_file("rpm2kmodkit");
    paths.copy_file("rpm2migrations");
    paths.copy_file("metadata.spec");

    // Create tarball in memory.
    println!("Starting tarball creation at {:?}", SystemTime::now());
    let mut buf_writer = Vec::new().writer();
    let enc = ZlibEncoder::new(&mut buf_writer, Compression::default());
    let mut tar = tar::Builder::new(enc);
    tar.append_dir_all("", &paths.prep_dir).unwrap();

    // Drop tar object to ensure any finalizing steps are done.
    drop(tar);

    // Get a reference to the tarball bytes.
    let tar_gz_data = buf_writer.get_ref();
    println!("tar_gz is {} megabytes", tar_gz_data.len() / 1024);

    // Write the tarball to the OUT_DIR where it can be imported during the build.
    fs::write(&paths.tar_gz, tar_gz_data).expect(&format!(
        "Unable to write to file '{}'",
        paths.tar_gz.display()
    ));
    println!("Done at {:?}", SystemTime::now());
}

struct Paths {
    /// The directory where our scripts, Makefile.toml etc. are located.
    data_input_dir: PathBuf,
    /// The directory that we will copy everything to before creating a tarball.
    prep_dir: PathBuf,
    /// The path to tools.tar.gz
    tar_gz: PathBuf,
}

impl Paths {
    fn new() -> Self {
        // This is the directory that cargo creates for us so that we can pass things from the build
        // script to the main compilation phase.
        let out_dir =
            PathBuf::from(env::var("OUT_DIR").expect("The cargo variable 'OUT_DIR' is missing"));

        Self {
            data_input_dir: PathBuf::from(DATA_INPUT_DIR),
            prep_dir: out_dir.join("tools"),
            tar_gz: out_dir.join("tools.tar.gz"),
        }
    }

    /// Copy a file from the `data_input_dir` to the `prep_dir`.
    fn copy_file(&self, filename: &str) {
        copy_file_impl(
            self.data_input_dir.join(filename),
            self.prep_dir.join(filename),
        );
    }
}

// Copy a file and provide a useful error message if it fails.
fn copy_file_impl<P1, P2>(source: P1, dest: P2)
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    let source = source.as_ref();
    let dest = dest.as_ref();
    fs::copy(source, dest).expect(&format!(
        "Unable to copy `{}' to '{}'",
        source.display(),
        dest.display()
    ));
}
