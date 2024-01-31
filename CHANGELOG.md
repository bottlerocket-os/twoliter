# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.6] - 2024-01-30

### Added

- Add Go and Proxy environment variables to those that `twoliter make` passes through ([#127])
- Add test type for cluster templating in Testsys ([#137])
- Enable a custom lookaside cache when building packages ([#141])

### Changed

- Fix an issue where Twoliter could fail if the tools directory does not pre-exist ([#138])
- Fix a documentation issue in the README. Thank you, @krispage ([#143])
- Update testsys to v0.11.0 ([#149])

[#127]: https://github.com/bottlerocket-os/twoliter/pull/127
[#137]: https://github.com/bottlerocket-os/twoliter/pull/137
[#138]: https://github.com/bottlerocket-os/twoliter/pull/138
[#141]: https://github.com/bottlerocket-os/twoliter/pull/141
[#143]: https://github.com/bottlerocket-os/twoliter/pull/143
[#149]: https://github.com/bottlerocket-os/twoliter/pull/149

## [0.0.5] - 2024-01-10

### Added

- Add alpha version of build variant command ([#119], [#108], [#106], [#105], [#97])

### Changed

- Provide better error messages for some filesystem operations ([#129])
- Deprecate the use of Release.toml ([#126], [#112])
- Install twoliter tools into a fixed directory ([#102])
- Update dependencies ([#125], [#98], [#93])
- Fix a bug that prevented use of a log level argument with testsys ([#92])

[#92]: https://github.com/bottlerocket-os/twoliter/pull/92
[#93]: https://github.com/bottlerocket-os/twoliter/pull/93
[#97]: https://github.com/bottlerocket-os/twoliter/pull/97
[#98]: https://github.com/bottlerocket-os/twoliter/pull/98
[#102]: https://github.com/bottlerocket-os/twoliter/pull/102
[#105]: https://github.com/bottlerocket-os/twoliter/pull/105
[#106]: https://github.com/bottlerocket-os/twoliter/pull/106
[#108]: https://github.com/bottlerocket-os/twoliter/pull/108
[#112]: https://github.com/bottlerocket-os/twoliter/pull/112
[#119]: https://github.com/bottlerocket-os/twoliter/pull/119
[#125]: https://github.com/bottlerocket-os/twoliter/pull/125
[#126]: https://github.com/bottlerocket-os/twoliter/pull/126
[#129]: https://github.com/bottlerocket-os/twoliter/pull/129

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

[unreleased]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.6...HEAD
[0.0.6]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/bottlerocket-os/twoliter/releases/tag/v0.0.1
