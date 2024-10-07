# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

[unreleased]: https://github.com/bottlerocket-os/twoliter/compare/v0.4.7...HEAD

## [0.4.7] - 2024-10-07

### Added

- Add support for building with erofs root filesystems ([#379])

### Fixed

- Refrain from tracking `BUILDSYS_VARIANT` environment variable in change-detection ([#377])
- Generate `/usr/share/bottlerocket` if not created by any variant packages ([#381])
- Fix kit publication not fully-overriding OCI repository names ([#385])

[#377]: https://github.com/bottlerocket-os/twoliter/pull/377
[#379]: https://github.com/bottlerocket-os/twoliter/pull/379
[#381]: https://github.com/bottlerocket-os/twoliter/pull/381
[#385]: https://github.com/bottlerocket-os/twoliter/pull/385

## [0.4.6] - 2024-09-16

### Changed

- Add support for vendor override files ([#344])
- Updated buildsys to add new 'build-all' target, reduce build time ([#345], [#357])
- CICD, workspace and doc improvements ([#353], [#354], [#355], [#358])
- Add support for partial lockfile validation & refactor lock interfaces, improve logging ([#361], [#363], [#370])
- Update tough dependencies to latest versions ([#365])
- Drop variant argument for variant subcommands ([#369])
- Add support for publishing kits to repositories that do not share a name with the kit ([#372])

[#344]: https://github.com/bottlerocket-os/twoliter/pull/344
[#345]: https://github.com/bottlerocket-os/twoliter/pull/345
[#353]: https://github.com/bottlerocket-os/twoliter/pull/353
[#354]: https://github.com/bottlerocket-os/twoliter/pull/354
[#355]: https://github.com/bottlerocket-os/twoliter/pull/355
[#357]: https://github.com/bottlerocket-os/twoliter/pull/357
[#358]: https://github.com/bottlerocket-os/twoliter/pull/358
[#361]: https://github.com/bottlerocket-os/twoliter/pull/361
[#363]: https://github.com/bottlerocket-os/twoliter/pull/363
[#365]: https://github.com/bottlerocket-os/twoliter/pull/365
[#369]: https://github.com/bottlerocket-os/twoliter/pull/369
[#370]: https://github.com/bottlerocket-os/twoliter/pull/370
[#372]: https://github.com/bottlerocket-os/twoliter/pull/372

[0.4.6]: https://github.com/bottlerocket-os/twoliter/compare/v0.4.5...v0.4.6

## [0.4.5] - 2024-08-22

### Changed

- Update twoliter to re-resolve workspaces at buildtime to detect lock mismatches ([#337])
- Improve logging in twoliter lockfile resolution ([#338])
- Improve error messages on pubsys SSM parameter validation failure ([#348])
- Improve reliability of pubsys SSM parameter validation with client-side rate-limiting and retries ([#348])

[#337]: https://github.com/bottlerocket-os/twoliter/pull/337
[#338]: https://github.com/bottlerocket-os/twoliter/pull/338
[#348]: https://github.com/bottlerocket-os/twoliter/pull/348

[0.4.5]: https://github.com/bottlerocket-os/twoliter/compare/v0.4.4...v0.4.5

## [0.4.4] - 2024-08-02

### Changed

- Update cross to newer version ([#328])
- Update testsys to v0.0.14 ([#341])
- imghelper: remove full path from .vmlinuz.hmac ([#336])
- imghelper: add ShellCheck exception to undo_sign() ([#336])
- imghelper: hoist AWS vars into global environment ([#340])
- TestSys: update log reader to use AsyncBufRead ([#341])
- rpm2img: use latest rpm release for inventory ([#342])

[#328]: https://github.com/bottlerocket-os/twoliter/pull/328
[#336]: https://github.com/bottlerocket-os/twoliter/pull/336
[#340]: https://github.com/bottlerocket-os/twoliter/pull/340
[#341]: https://github.com/bottlerocket-os/twoliter/pull/341
[#342]: https://github.com/bottlerocket-os/twoliter/pull/342

[0.4.4]: https://github.com/bottlerocket-os/twoliter/compare/v0.4.3...v0.4.4

## [0.4.3] - 2024-07-17

### Changed

- Update rust nightly to newer version ([#325])
- Fix image handling bugs in `twoliter update` ([#326])

[#325]: https://github.com/bottlerocket-os/twoliter/pull/325
[#326]: https://github.com/bottlerocket-os/twoliter/pull/326

[0.4.3]: https://github.com/bottlerocket-os/twoliter/compare/v0.4.2...v0.4.3

## [0.4.2] - 2024-07-11

### Added

- Add support for crane family of tools for publishing and fetching kits ([#305], [#317])
- Add additional image feature flags ([#318])

### Changed

- Update application inventory generation to accommodate kits ([#310])
- Share file descriptors to the build container to speed up directory I/O ([#302])
- Combine build and repack dockerfiles ([#302])
- Move updater wave default schedules into pubsys ([#321])
- Drop support for cgroup feature flags ([#318])

[#302]: https://github.com/bottlerocket-os/twoliter/pull/302
[#305]: https://github.com/bottlerocket-os/twoliter/pull/305
[#310]: https://github.com/bottlerocket-os/twoliter/pull/310
[#317]: https://github.com/bottlerocket-os/twoliter/pull/317
[#318]: https://github.com/bottlerocket-os/twoliter/pull/318
[#321]: https://github.com/bottlerocket-os/twoliter/pull/321

[0.4.2]: https://github.com/bottlerocket-os/twoliter/compare/v0.4.1...v0.4.2

## [0.4.1] - 2024-06-19

### Changed

- Stop printing `ManifestParse` during builds ([#300])
- Revert application-inventory: use RPM's Version and Release, set Epoch ([#301])
- Fix issue in rpm2kit by using awk instead of head ([#303])
- Application-inventory: use core-kit version for packages sourced from the bottlerocket-core-kit ([#304])
- Add a pull in Twoliter to allow inspecting the image config ([#306])
- Fix purge go-vendor task in Twoliter ([#307])

[#300]: https://github.com/bottlerocket-os/twoliter/pull/300
[#301]: https://github.com/bottlerocket-os/twoliter/pull/301
[#303]: https://github.com/bottlerocket-os/twoliter/pull/303
[#304]: https://github.com/bottlerocket-os/twoliter/pull/304
[#306]: https://github.com/bottlerocket-os/twoliter/pull/306
[#307]: https://github.com/bottlerocket-os/twoliter/pull/307

[0.4.1]: https://github.com/bottlerocket-os/twoliter/compare/v0.4.0...v0.4.1

## [0.4.0] - 2024-06-17

### Changed

- Save each package in its own layer for external kits in Twoliter ([#297])
- Docker pull before docker save for external kits in Twoliter ([#298])

[#297]: https://github.com/bottlerocket-os/twoliter/pull/297
[#298]: https://github.com/bottlerocket-os/twoliter/pull/298

[0.4.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.3.0...v0.4.0

## [0.3.0] - 2024-06-13

### Added

- Add external kit support ([#230])
- Add a subcommand to build kits ([#249])
- Add Twoliter.lock creation and resolution ([#250])
- Add Twoliter fetch command ([#270])
- Add ability to generate kit metadata and create OCI image ([#271])
- Add external kits test project and kit repo discovery ([#272])

[#230]: https://github.com/bottlerocket-os/twoliter/pull/230
[#249]: https://github.com/bottlerocket-os/twoliter/pull/249
[#250]: https://github.com/bottlerocket-os/twoliter/pull/250
[#270]: https://github.com/bottlerocket-os/twoliter/pull/270
[#271]: https://github.com/bottlerocket-os/twoliter/pull/271
[#272]: https://github.com/bottlerocket-os/twoliter/pull/272

[0.3.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.2.0...v0.3.0

### Changed

- Dependabot: update h2, rustls versions ([#212], [#213])
- Remove support for license overrides ([#241])
- Use grub-bios-setup from the SDK ([#242])
- Changes and fixes to better support kits ([#245], [#246], [#265], [#266], [#268], [#278], [#285], [#287], [#288], [#290], [#291], [#292], [#293], [#294], [#296])
- Add cargo-metadata dependency to repack-variant ([#260])
- Remove fetch-licenses from build kit ([#261])
- Change the way application inventory is created ([#263])
- Twoliter cleanup and fixes ([#274], [#275], [#276], [#280], [#283], [#295])
- Remove variant based sensitivity ([#282])
- Remove Alpha SDK usage in twoliter build variant ([#286])

[#241]: https://github.com/bottlerocket-os/twoliter/pull/241
[#242]: https://github.com/bottlerocket-os/twoliter/pull/242
[#245]: https://github.com/bottlerocket-os/twoliter/pull/245
[#246]: https://github.com/bottlerocket-os/twoliter/pull/246
[#260]: https://github.com/bottlerocket-os/twoliter/pull/260
[#261]: https://github.com/bottlerocket-os/twoliter/pull/261
[#263]: https://github.com/bottlerocket-os/twoliter/pull/263
[#265]: https://github.com/bottlerocket-os/twoliter/pull/265
[#266]: https://github.com/bottlerocket-os/twoliter/pull/266
[#268]: https://github.com/bottlerocket-os/twoliter/pull/268
[#274]: https://github.com/bottlerocket-os/twoliter/pull/274
[#275]: https://github.com/bottlerocket-os/twoliter/pull/275
[#276]: https://github.com/bottlerocket-os/twoliter/pull/276
[#278]: https://github.com/bottlerocket-os/twoliter/pull/278
[#280]: https://github.com/bottlerocket-os/twoliter/pull/280
[#282]: https://github.com/bottlerocket-os/twoliter/pull/282
[#283]: https://github.com/bottlerocket-os/twoliter/pull/283
[#285]: https://github.com/bottlerocket-os/twoliter/pull/285
[#286]: https://github.com/bottlerocket-os/twoliter/pull/286
[#287]: https://github.com/bottlerocket-os/twoliter/pull/287
[#288]: https://github.com/bottlerocket-os/twoliter/pull/288
[#290]: https://github.com/bottlerocket-os/twoliter/pull/290
[#291]: https://github.com/bottlerocket-os/twoliter/pull/291
[#292]: https://github.com/bottlerocket-os/twoliter/pull/292
[#293]: https://github.com/bottlerocket-os/twoliter/pull/293
[#294]: https://github.com/bottlerocket-os/twoliter/pull/294
[#295]: https://github.com/bottlerocket-os/twoliter/pull/295
[#296]: https://github.com/bottlerocket-os/twoliter/pull/296

## [0.2.0] - 2024-05-29

### Added

- Add support to repack a variant ([#214], [#211], [#217], [#219], [#221], [#222], [#228], [#231], [#235], [#243])
- Added the buildsys build-kit command to create kits ([#198], [#227])
- Add support to install CA certificates during image build ([#232])
- Add support to fetch a variant ([#236])

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
[#236]: https://github.com/bottlerocket-os/twoliter/pull/236
[#243]: https://github.com/bottlerocket-os/twoliter/pull/243

[0.2.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.1.1...v0.2.0

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

[0.1.1]: https://github.com/bottlerocket-os/twoliter/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.7...v0.1.0
[0.0.7]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.6...v0.0.7
[0.0.6]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/bottlerocket-os/twoliter/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/bottlerocket-os/twoliter/releases/tag/v0.0.1
