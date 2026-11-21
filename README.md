# Twoliter

A build tool for creating custom [Bottlerocket] variants.
See the [design doc] describing how Twoliter works.
We welcome ideas and requirements in the form of issues and comments!

[Bottlerocket]: https://github.com/bottlerocket-os/bottlerocket
[design doc]: docs/design/README.md

## Verifying builds

`twoliter verify ami` and `twoliter verify kit` rebuild a published AMI or kit from the source it
claims and compare the result against what was published. See
[How `twoliter verify` works](docs/verify.md) for the flow, and
[Verifying a kit](docs/verify-kit.md) for `verify kit`.

### Verifying an AMI

`twoliter verify ami --ami-id <id>` compares the rebuilt disk image against the AMI's snapshots
region by region. `--source-dir` builds in an existing clone instead of a fresh one; it must have
no uncommitted changes and no untracked or ignored files, such as the output of an earlier build
(see [Source checkout](docs/verify.md#source-checkout)). A rebuild cannot be byte-identical, since
each build signs with fresh keys, so each region has an explicit list of expected differences and
anything else fails:

| Region | Rule |
|---|---|
| GPT, BIOS-boot, DATA, data image | Byte-identical. B-bank, RESERVED and padding must be zero. |
| PRIVATE | If bytes differ, its files are compared: identical files make the difference expected, and any other difference fails, with lines of `bootconfig.data` found on only one side shown in the report. Sources pinning a twoliter older than 0.28.0 build ext4 partitions that depend on the build host (SELinux labels, and the directory hash flag on aarch64), so `verify ami` warns when the host would change PRIVATE. |
| ROOT-A (erofs only) | If bytes differ, compare the file trees: type, mode, owner, symlink target, xattrs (SELinux labels, capabilities), contents. Other root filesystem formats, and device nodes, are not supported. Kernel modules come from the kit already signed, so they must match like any other file, as must the TUF root role (`/usr/share/updog/root.json`): the rebuild embeds the one given with `--root-role`. Expected: the SBOMs (`/usr/share/bottlerocket/{spdx,cyclonedx}-sbom.json`), since sbomtool's output is not reproducible yet. |
| BOOT-CONFIG (`grub.cfg` in BOOT-A, or for UKI images, which have no BOOT-A, the UKI's `.cmdline` section in EFI-A) | Identical except the dm-verity root hash, salt and data size; exactly one verity table for the same devices. |
| HASH-A | Not compared: it is formatted with a random salt, so it never matches the rebuilt one. The root hash and salt it is built with are the fields BOOT-CONFIG allows to differ. |
| EFI-A, BOOT-A | If bytes differ, file by file, and every difference needs a listed cause or verification fails: a signature or certificate that both images have; a PE binary whose Authenticode digest matches, or that differs only in its key-holding sections (`.vendor_cert`, `.pubkey`) with the same headers and layout; a FIPS `.hmac` that is the HMAC of the published file it names; the verity root hash, salt and data size in `grub.cfg` or the UKI's `.cmdline`. Files in only one image fail, which also keeps out the add-ons, credentials and extension images systemd-stub would load next to a UKI. The kernel comes from the kit, so it must match apart from its signature. |

## For Maintainers

This section includes information for maintainers about testing and releasing Twoliter.

## Unit Tests and Integration Tests

If you run `cargo test` you will get the default features which includes the feature `integ-tests`.
These will be quite slow as some of them do complete builds.
If you don't have time for that, run `cargo test --no-default-features` to run only the fast tests.

## Testing the Binary in a Project

In general, if you have changes to Twoliter and want to try them out in a Twoliter project, it is as simple as building the Twoliter binary and using it in your project.
Different projects will have different ways of making sure the correct Twoliter binary is being used.
For example, Bottlerocket has a script and Makefile.toml variables that ensure the correct version of Twoliter is being used to build Bottlerocket.
The following sections describe how to use those mechanisms to use a non-released version of Twoliter.

### Testing Twoliter Changes in the Bottlerocket Repo

The process of testing changes to Twoliter in Bottlerocket is as follows:
- Commit your changes. This can be either a local commit or a commit on a git fork.
- In the Bottlerocket git repo
  - Remove the existing Twoliter binary if it exists.
  - Ensure the [nightly rust toolchain] is installed and enabled.
  - Run any/all `cargo make` commands with the following environment
    variables.

[nightly rust toolchain]: https://rust-lang.github.io/rustup/concepts/channels.html#working-with-nightly-rust

```sh
# The URL to the Twoliter git repository. This can be anything that git remote add would accept.
# Here we are using a path on the local filesystem.
TWOLITER_REPO=file:///home/myuser/repos/twoliter
# The sha of the commit you want to use. Make sure changes have been committed!
TWOLITER_VERSION=a8b30def
# These need to be set as follows.
TWOLITER_ALLOW_SOURCE_INSTALL=true
TWOLITER_ALLOW_BINARY_INSTALL=false
TWOLITER_SKIP_VERSION_CHECK=true
```

So, for example, to build a Bottlerocket image using a commit on a fork, I would look like this:

```sh
  rm -rf tools/twoliter
  cargo make -e=TWOLITER_REPO=https://github.com/webern/twoliter \
    -e=TWOLITER_VERSION=11afef09 \
    -e=TWOLITER_ALLOW_SOURCE_INSTALL=true \
    -e=TWOLITER_ALLOW_BINARY_INSTALL=false \
    -e=TWOLITER_SKIP_VERSION_CHECK=true \
  build-variant
```

### Alternative Method of Testing in the Bottlerocket Repo

Another way you can test your Twoliter changes in the Bottlerocket repo is by building twoliter and
moving it into the right place, then telling `cargo make` not to install Twoliter at all.

For example, if you have set `TWOLITER_REPO` and `BOTTLEROCKET_REPO` to the respective local git repositories, then:

```shell
cd $TWOLITER_REPO
cargo build --release --package twoliter --bin twoliter
rm -rf $BOTTLEROCKET_REPO/tools/twoliter
mkdir -p $BOTTLEROCKET_REPO/tools/twoliter
cp $TWOLITER_REPO/target/release/twoliter $BOTTLEROCKET_REPO/tools/twoliter

cd $BOTTLEROCKET_REPO

cargo make \
    -e=TWOLITER_ALLOW_SOURCE_INSTALL=false \
    -e=TWOLITER_ALLOW_BINARY_INSTALL=false \
    -e=TWOLITER_SKIP_VERSION_CHECK=true \
    build-variant
```

## Releasing

A release consists of a semver tag in the form `v0.0.0`.
We also use release-candidate tags in the form `v0.0.0-rc1`.

We use a fork of `cargo-dist` to facilitate binary releases.
The purpose of the `cargo-dist` fork is to enable cross-compilation with `cross`
We do not release Twoliter into `crates.io`.

`cargo-dist` is run by GitHub Actions, and the release workflow requires an additional approver to run the release action.

To perform a release:

- Create and merge a PR that bumps the version and changelog like [this one].
  This commit should use the desired `rc` version for the `twoliter` crate, e.g. `0.0.4-rc1`.
- Push a release-candidate tag matching the selected version, e.g. `v0.0.4-rc1`.
- That will create a GitHub Actions workflow which will create a GitHub release and attach binaries.
- An additional approver must navigate to the [Actions tab] and approve the Release workflow.
- Create a Bottlerocket PR ([example]) that uses the new version of Twoliter.
  At first, your PR will use the candidate tag.
  Before merging to the Bottlerocket repo, you will use the final release tag.
- Test Twoliter in Bottlerocket
  - `cargo make`
  - `cargo make ami`
  - `cargo make test --help`
  - More extensive testing if needed.
  - Note: If you want to use your local version of Twoliter to build Bottlerocket.
    - Delete the existing install of Twoliter if it exists.
    - Commit and push your code to your fork of Twoliter. For example: https://github.com/YOUR_GIT_ALIAS/twoliter
    - Set the following Environment variables
      - TWOLITER_REPO = Your Twoliter Repository link.
      - TWOLITER_VERSION = Hash of the commit pushed to your Twoliter repository.
      - TWOLITER_ALLOW_SOURCE_INSTALL = True, as we want to do a source install.
      - TWOLITER_ALLOW_BINARY_INSTALL = False, as we do want to use a Twoliter binary.
      - TWOLITER_SKIP_VERSION_CHECK = True, as we do not want to use a certain Twoliter binary version.
  - For example,
```
  rm -rf tools/twoliter
  cargo make -e=TWOLITER_REPO=https://github.com/YOUR_GIT_ALIAS/twoliter \
    -e=TWOLITER_VERSION=HASH_OF_COMMIT \
    -e=TWOLITER_ALLOW_SOURCE_INSTALL=true \
    -e=TWOLITER_ALLOW_BINARY_INSTALL=false \
    -e=TWOLITER_SKIP_VERSION_CHECK=true
```
- When it's working, create a new PR to the Twoliter repo dropping the `-rc` in the version string.
- Merge the Twoliter PR and push a finalized tag, e.g. `v0.0.4`.
- An additional approver must navigate to the [Actions tab] and approve the Release workflow.
- Once the GitHub Actions workflow finishes, update the Bottlerocket PR to your finalized tag.
- Merge the Bottlerocket PR

[this one]: https://github.com/bottlerocket-os/twoliter/pull/91
[example]: https://github.com/bottlerocket-os/bottlerocket/pull/3480
[Actions tab]: https://github.com/bottlerocket-os/twoliter/actions
