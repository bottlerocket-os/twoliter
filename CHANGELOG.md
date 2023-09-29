# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.4] - 2023-10-04

### Added

- Enable log level selection for Testsys and Pubsys through Twoliter ([#75])
- Enable specification of Bottlerocket SDK in Twoliter.toml ([#89])

### Changed

- Testsys: add image_id label to fix metal cluster conflicts ([#81])
- Testsys: change update cluster shared security group name ([#67])
- Testsys: Update version to v0.10.0 ([#93])
- Remove Infrasys, an unused system, from the codebase ([#53]) 

[#53]: https://github.com/bottlerocket-os/twoliter/pull/53
[#67]: https://github.com/bottlerocket-os/twoliter/pull/67
[#75]: https://github.com/bottlerocket-os/twoliter/pull/75
[#81]: https://github.com/bottlerocket-os/twoliter/pull/81
[#89]: https://github.com/bottlerocket-os/twoliter/pull/89
[#93]: https://github.com/bottlerocket-os/twoliter/pull/93

## [0.0.3] - 2023-09-13

### Added

- Bottlerocket build system tools:
  - `buildsys`
  - `pubsys`
  - `pubsys-setup`
  - `testsys`
  - `scripts`
  - `Dockerfile`
- Add `cargo dist` for binary releases.

### Changed

- Update docker run commands to use current `--security-opt` syntax.

## [0.0.2] - 2023-08-18

### Changed

- Removed keys from the project file schema since they are not yet being used.

## [0.0.1] - 2023-08-17

### Added

- The `twoliter` CLI with a command, `twoliter make`, which serves as a facade over
  Bottlerocket's `cargo make` build system.
- `Makefile.toml` taken from the Bottlerocket project.

[unreleased]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.4...HEAD
[0.0.4]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/bottlerocket-os/twoliter/releases/tag/v0.0.1
