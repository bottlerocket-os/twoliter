/*!

Prepare and package embedded tools in a tarball to be included with Twoliter.

!*/

// The performance cost of this is infinitesimal, and we get a better panic stack with `expect`.
#![allow(clippy::expect_fun_call)]

use bytes::BufMut;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::path::{Path, PathBuf};
use std::{env, fs};

const DATA_INPUT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/embedded");

fn main() {
    println!("cargo:rerun-if-changed={}", DATA_INPUT_DIR);
    // Make sure we run again if the target triple (i.e. aarch64-unknown-linux-gnu) changes.
    println!("cargo:rerun-if-env-changed=TARGET");
    let data_input_dir = PathBuf::from(DATA_INPUT_DIR);

    // This is the directory that cargo creates for us so that we can pass things from the build
    // script to the main compilation phase.
    let out_dir =
        PathBuf::from(env::var("OUT_DIR").expect("The cargo variable 'OUT_DIR' is missing"));

    // This is where we will copy all of the things we want to add to our tarball. We will then
    // compress and tar this directory.
    let tools_dir = out_dir.join("tools");
    fs::create_dir_all(&tools_dir).expect(&format!(
        "Unable to create directory '{}'",
        tools_dir.display()
    ));

    // This is the filepath to the tarball we will create.
    let tar_path = out_dir.join("tools.tar.gz");

    copy_file(
        data_input_dir.join("Makefile.toml"),
        tools_dir.join("Makefile.toml"),
    );

    // Create tarball in memory.
    let mut buf_writer = Vec::new().writer();
    let enc = ZlibEncoder::new(&mut buf_writer, Compression::default());
    let mut tar = tar::Builder::new(enc);
    tar.append_dir_all("", &tools_dir).unwrap();

    // Drop tar object to ensure any finalizing steps are done.
    drop(tar);

    // Get a reference to the tarball bytes.
    let tar_gz_data = buf_writer.get_ref();

    // Write the tarball to the OUT_DIR where it can be imported during the build.
    fs::write(&tar_path, tar_gz_data)
        .expect(&format!("Unable to write to file '{}'", tar_path.display()));
}

// Copy a file and provide a useful error message if it fails.
fn copy_file<P1, P2>(source: P1, dest: P2)
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
