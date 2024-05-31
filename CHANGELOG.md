# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2024-05-29

### Added

- Add support to repack a variant ([#214], [#211], [#217], [#219], [#221], [#222], [#228], [#231], [#235], [#243])
- Added the buildsys build-kit command to create kits ([#198], [#227])
- Add support to install CA certificates during image build ([#232])

[#198]: https://github.com/bottlerocket-os/twoliter/pull/198
[#211]: https://github.com/bottlerocket-os/twoliter/pull/211
[#214]: https://github.com/bottlerocket-os/twoliter/pull/214
[#217]: https://github.com/bottlerocket-os/twoliter/pull/217
[#219]: https://github.com/bottlerocket-os/twoliter/pull/219
[#221]: https://github.com/bottlerocket-os/twoliter/pull/221
[#222]: https://github.com/bottlerocket-os/twoliter/pull/222
[#227]: https://github.com/bottlerocket-os/twoliter/pull/227
[#228]: https://github.com/bottlerocket-os/twoliter/pull/228
[#231]: https://github.com/bottlerocket-os/twoliter/pull/231
[#232]: https://github.com/bottlerocket-os/twoliter/pull/232
[#243]: https://github.com/bottlerocket-os/twoliter/pull/243

### Changed

- Regenerate kernel module if possible in rpm2img ([#205])
- Changes and fixes to better support kits ([#210], [#216], [#218], [#223], [#224], [#226], [#234], [#238])
- Deprecate variant sensitivity for packages in buildsys ([#220])
- Install 'root.json' during image build ([#239])
- Backward compatibility for existing projects ([#244])

[#205]: https://github.com/bottlerocket-os/twoliter/pull/205
[#210]: https://github.com/bottlerocket-os/twoliter/pull/210
[#216]: https://github.com/bottlerocket-os/twoliter/pull/216
[#218]: https://github.com/bottlerocket-os/twoliter/pull/218
[#220]: https://github.com/bottlerocket-os/twoliter/pull/220
[#223]: https://github.com/bottlerocket-os/twoliter/pull/223
[#224]: https://github.com/bottlerocket-os/twoliter/pull/224
[#226]: https://github.com/bottlerocket-os/twoliter/pull/226
[#234]: https://github.com/bottlerocket-os/twoliter/pull/234
[#235]: https://github.com/bottlerocket-os/twoliter/pull/235
[#238]: https://github.com/bottlerocket-os/twoliter/pull/238
[#239]: https://github.com/bottlerocket-os/twoliter/pull/239
[#244]: https://github.com/bottlerocket-os/twoliter/pull/244

## [0.1.1] - 2024-04-17

### Added

### Changed

- Use Openssl to generate HMAC in rpm2img ([#196])

[#196]: https://github.com/bottlerocket-os/twoliter/pull/196

## [0.1.0] - 2024-04-08

### Added

- Add FIPS-related functionality ([#181])
- Add build clean command ([#183])

[#181]: https://github.com/bottlerocket-os/twoliter/pull/181
[#183]: https://github.com/bottlerocket-os/twoliter/pull/183

### Changed

- Breaking Change: Switch to the unified SDK ([#166])
- Fixed Gomod.rs bug ([#178])
- Use Twoliter.toml for cache layers ([#179])
- Update readme ([#182, #184])
- Generate HMAC for kernel on build ([#187])

[#166]: https://github.com/bottlerocket-os/twoliter/pull/166
[#178]: https://github.com/bottlerocket-os/twoliter/pull/178
[#179]: https://github.com/bottlerocket-os/twoliter/pull/179
[#182]: https://github.com/bottlerocket-os/twoliter/pull/182
[#184]: https://github.com/bottlerocket-os/twoliter/pull/184
[#187]: https://github.com/bottlerocket-os/twoliter/pull/187

## [0.0.7] - 2024-03-19

### Added

- Testsys can now assume a role for workload tests ([#169])

### Changed

- Fix `--upstream-source-fallback` argument in `twoliter build variant` ([#168], thanks @tzneal)
- Fix a bug in pubsys resulting in a key generation error ([#165])
- Fix an issue with pubsys using the wrong environment variable for the SDK ([#157])
- Fix an issue in pubsys with trailing a lookaside cache URL having a trailing slash ([#159])
- Fix in the alpha SDK script and add dev packages ([#147], [#164])
- Update buildsys to use clap for environment variables ([#134])
- Refactor buildsys builder.rs logic ([#134], [#156])
- Update dependencies ([#171])

[#134]: https://github.com/bottlerocket-os/twoliter/pull/134
[#147]: https://github.com/bottlerocket-os/twoliter/pull/147
[#156]: https://github.com/bottlerocket-os/twoliter/pull/156
[#157]: https://github.com/bottlerocket-os/twoliter/pull/157
[#159]: https://github.com/bottlerocket-os/twoliter/pull/159
[#164]: https://github.com/bottlerocket-os/twoliter/pull/164
[#165]: https://github.com/bottlerocket-os/twoliter/pull/165
[#168]: https://github.com/bottlerocket-os/twoliter/pull/168
[#169]: https://github.com/bottlerocket-os/twoliter/pull/169
[#171]: https://github.com/bottlerocket-os/twoliter/pull/171

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

[unreleased]: https://github.com/bottlerocket-os/twoliter/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/bottlerocket-os/twoliter/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.7...v0.1.0
[0.0.7]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.6...v0.0.7
[0.0.6]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/bottlerocket-os/twoliter/releases/tag/v0.0.1
