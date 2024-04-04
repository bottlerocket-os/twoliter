# Twoliter

A build tool for creating [Bottlerocket] variants.

[Bottlerocket]: https://github.com/bottlerocket-os/bottlerocket

## 🚧👷 

This project is just getting started and is not ready for contributions.
Soon we will be posting a design document and creating GitHub issues.

We welcome ideas and requirements in the form of issues and comments!

## For Maintainers

This section includes information for maintainers about testing and releasing Twoliter.

### Release

A release consists of a semver tag in the form `v0.0.0`.
We also use release-candidate tags in the form `v0.0.0-rc1`.
Release-candidate tags are typically deleted after the release is done.

We use a fork of `cargo-dist` to facilitate binary releases.
The purpose of the `cargo-dist` fork is to enable cross-compilation with `cross`
We do not release Twoliter into `crates.io`.

To perform a release:

- Create a PR that bumps the version and changelog like [this one].
- Push a release-candidate tag, e.g. `v0.0.4-rc1`.
- That will kick of a GitHub Actions workflow that creates a GitHub release and attaches binaries.
- Create a Bottlerocket PR ([example]) that uses the new version of Twoliter.
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
