# Twoliter

A build tool for creating custom [Bottlerocket] variants.

[Bottlerocket]: https://github.com/bottlerocket-os/bottlerocket

## Status 🚧👷

This project is a work in progress and is not ready for outside contributors yet.
Issues marked as "good first issue" are intended for team members at this time.
There is a design doc describing how Twoliter is intended to be built, [here]

We welcome ideas and requirements in the form of issues and comments!

[here]: docs/design/README.md

## For Maintainers

This section includes information for maintainers about testing and releasing Twoliter.

## Testing

In general, if you have changes to Twoliter and want to try them out in a Twoliter project, it is as simple as building the Twoliter binary and using it in your project.
Different projects will have different ways of making sure the correct Twoliter binary is being used.
For example, Bottlerocket has a script and Makefile.toml variables that ensure the correct version of Twoliter is being used to build Bottlerocket.
The following sections describe how to use those mechanisms to use a non-released version of Twoliter.

### Testing Twoliter Changes in the Bottlerocket Repo

The process of testing changes to Twoliter in Bottlerocket is as follows:
- Commit your changes. This can be either a local commit or a commit on a git fork.
- In the Bottlerocket git repo
  - Remove the existing Twoliter binary if it exists.
  - Run any/all `cargo make` commands with the following environment
    variables.

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

To perform a release:

- Create a PR that bumps the version and changelog like [this one].
- Push a release-candidate tag, e.g. `v0.0.4-rc1`.
- That will kick of a GitHub Actions workflow that creates a GitHub release and attaches binaries.
- Create a Bottlerocket PR ([example]) that uses the new version of Twoliter.
  At first, your PR will use the candidate tag.
  Before merging, you will use the final release tag.
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
- When it's working merge the Twoliter PR and push a finalized tag, e.g. `v0.0.4`.
- Once the GitHub Actions workflow finishes, update the Bottlerocket PR to your finalized tag.
- Merge the Bottlerocket PR
- Delete your release-candidates and release-candidate tags from the GitHub repository (using the GitHub UI).

[this one]: https://github.com/bottlerocket-os/twoliter/pull/91
[example]: https://github.com/bottlerocket-os/bottlerocket/pull/3480
