# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

[unreleased]: https://github.com/bottlerocket-os/twoliter/compare/v0.18.0...HEAD

## [0.18.0] - 2026-04-02

### Added
* Add ability to override variant properties in bottlerocket-variant ([#647])

### Build
* Bump actions/setup-go from 6.2.0 to 6.3.0 ([#644])
* Update cargo dependencies ([#646], [#648], [#650], [#651], [#652])

[#644]: https://github.com/bottlerocket-os/twoliter/pull/644
[#646]: https://github.com/bottlerocket-os/twoliter/pull/646
[#647]: https://github.com/bottlerocket-os/twoliter/pull/647
[#648]: https://github.com/bottlerocket-os/twoliter/pull/648
[#650]: https://github.com/bottlerocket-os/twoliter/pull/650
[#651]: https://github.com/bottlerocket-os/twoliter/pull/651
[#652]: https://github.com/bottlerocket-os/twoliter/pull/652

[0.18.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.17.0....v0.18.0


## [0.17.0] - 2026-02-26

### Added
* Add pcrsys to predict PCR values ([#625])
* Add --quiet flag to reduce build output ([#607])
* Add check-advisories to validate BRSA fields ([#604])

### Changed
* Update design documentation ([#610])
* Update Rust nightly toolchain and fix rand features for buildsys ([#624])
* Bump source package anchor version ([#637])

### Fixed
* Add source packages of sub-packages in rpm2img for application-inventory generation ([#606])

### Build
* Add e2e test for build-variant and repack-variant build targets ([#623])
* Update cargo dependencies ([#632], [#630], [#631], [#640], [#633])

[#604]: https://github.com/bottlerocket-os/twoliter/pull/604
[#606]: https://github.com/bottlerocket-os/twoliter/pull/606
[#607]: https://github.com/bottlerocket-os/twoliter/pull/607
[#610]: https://github.com/bottlerocket-os/twoliter/pull/610
[#623]: https://github.com/bottlerocket-os/twoliter/pull/623
[#624]: https://github.com/bottlerocket-os/twoliter/pull/624
[#625]: https://github.com/bottlerocket-os/twoliter/pull/625
[#630]: https://github.com/bottlerocket-os/twoliter/pull/630
[#631]: https://github.com/bottlerocket-os/twoliter/pull/631
[#632]: https://github.com/bottlerocket-os/twoliter/pull/632
[#633]: https://github.com/bottlerocket-os/twoliter/pull/633
[#637]: https://github.com/bottlerocket-os/twoliter/pull/637
[#640]: https://github.com/bottlerocket-os/twoliter/pull/640

[0.17.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.16.0...v0.17.0


## [0.16.0] - 2026-01-06

### Added
* Add sbomtool step to generate an SBOM of the RPMs in an image for use in the SBOM merge ([#627])
* Add AWS SDK messages to errors ([#613])

### Fixed
* Fixed non-EROFS variants to properly remove uncompressed SBOM after compression ([#627])

[#613]: https://github.com/bottlerocket-os/twoliter/pull/613
[#627]: https://github.com/bottlerocket-os/twoliter/pull/627

[0.16.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.15.1...v0.16.0


## [0.15.1] - 2025-12-17

### Fixed
* Fix sbom_package_dir optional in imghelper ([#618])

[#618]: https://github.com/bottlerocket-os/twoliter/pull/618

[0.15.1]: https://github.com/bottlerocket-os/twoliter/compare/v0.15.0...v0.15.1

## [0.15.0] - 2025-12-11

**Note: This release has known issues with SBOM package directory handling. Please see https://github.com/bottlerocket-os/twoliter/issues/619 for details**

### Changed
* Consolidate SBOM packages into a single merged SBOM ([#583])
* Update Bottlerocket SDK to version 0.66.0 ([#583])

[#583]: https://github.com/bottlerocket-os/twoliter/pull/583

[0.15.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.14.0...v0.15.0

## [0.14.0] - 2025-12-08

### Changed
* Move `fetch-vendored` to use `go-latest` for fetching Go dependencies ([#605])

### Added
* Add version transition check to `check-migrations` task to validate all version transitions are present in `Release.toml` ([#598])

### Fixed
* Re-enable all tests on `make test` by removing `default-members` from workspace ([#608])
* Sort AMI tags by key for deterministic output in amispec ([#608])

### Build
* Handle release profile in include-env-compressed test ([#608])

[#598]: https://github.com/bottlerocket-os/twoliter/pull/598
[#605]: https://github.com/bottlerocket-os/twoliter/pull/605
[#608]: https://github.com/bottlerocket-os/twoliter/pull/608

[0.14.0-rc1]: https://github.com/bottlerocket-os/twoliter/compare/v0.13.0...v0.14.0-rc1

## [0.13.0] - 2025-11-10

### Changed
* Remove experimental label from `erofs-root-partition` feature ([#586])
* Sort bootconfig keys across all snippets for consistent output ([#596])

### Added
* Add experimental `encrypted-storage` image feature for encrypting local storage using TPM2 devices ([#589])
* Record in-place updates enablement in `image-features.env` for runtime queries ([#589])
* Emit `artifact-metadata.json` with disk utilization for ROOT and BOOT partitions ([#581])

### Fixed
* Fix pubsys to copy image tags when replicating AMIs ([#592])
* Prevent re-packing non-erofs images with EROFS ([#576])

### Deprecated
* Deprecate `systemd-networkd` image feature ([#589])
* Deprecate `grub-set-private-var` image feature ([#589])

### Build
* Update cargo dependencies ([#591])
* Bump actions/checkout from 4.2.2 to 5.0.0 ([#569])
* Fail the build if the go compiler fails in krane-bundle ([#568])
* Update ecr-login to v0.11.0 ([#595])

### Documentation
* Document and add tests for AMI tagging behavior ([#590])
* Update README ([#588], thanks @webern)

### Improved
* TestSys: Run workload tests for different NVIDIA flavors ([#594])

[#568]: https://github.com/bottlerocket-os/twoliter/pull/568
[#569]: https://github.com/bottlerocket-os/twoliter/pull/569
[#576]: https://github.com/bottlerocket-os/twoliter/pull/576
[#581]: https://github.com/bottlerocket-os/twoliter/pull/581
[#586]: https://github.com/bottlerocket-os/twoliter/pull/586
[#588]: https://github.com/bottlerocket-os/twoliter/pull/588
[#589]: https://github.com/bottlerocket-os/twoliter/pull/589
[#590]: https://github.com/bottlerocket-os/twoliter/pull/590
[#591]: https://github.com/bottlerocket-os/twoliter/pull/591
[#592]: https://github.com/bottlerocket-os/twoliter/pull/592
[#594]: https://github.com/bottlerocket-os/twoliter/pull/594
[#595]: https://github.com/bottlerocket-os/twoliter/pull/595
[#596]: https://github.com/bottlerocket-os/twoliter/pull/596

[0.13.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.12.0...v0.13.0

## [0.12.0] - 2025-08-21

### Breaking Changes
* Update schema-version of Twoliter.toml to version 2. This introduces a new field project-vendor. ([#551])
  * Users will need to run `twoliter update` after updating twoliter on any kits or variant repositories to migrate their lockfile to schema-version 2.
  * Users will need to bump the schema-version to 2 in Twoliter.toml if they want to use the new project-vendor field

### Fixed
* Fix a check that prevent inconsistent manual change in twoliter.lock ([#563])

### Changed
* Always fetch kit deps before build tasks ([#565])

### Improved
* Extract kits concurrently ([#566])

### Build
* Update cargo dependencies ([#570])

[#551]: https://github.com/bottlerocket-os/twoliter/pull/551
[#563]: https://github.com/bottlerocket-os/twoliter/pull/563
[#565]: https://github.com/bottlerocket-os/twoliter/pull/565
[#566]: https://github.com/bottlerocket-os/twoliter/pull/566
[#570]: https://github.com/bottlerocket-os/twoliter/pull/570

[0.12.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.11.0...v0.12.0

## [0.11.0] - 2025-07-17

### Fixed
* Fix race condition in bypass container launching ([#553])

### Added
* Add support to delete individual tests in testsys ([#549])
* Add `--dry-run` mode to `pubsys promote-ssm` ([#529])
* Allow keys from environment variables ([#533])

### Changed
* Use "fat" LTO for dist and "thin" for release ([#535])
* Compress included tool archives ([#537])
* Use "profile-hint-mostly-unused" for AWS SDKs ([#555])

### Improved
* Reclaim reserved space on ext4 filesystems ([#550])

[#529]: https://github.com/bottlerocket-os/twoliter/pull/529
[#533]: https://github.com/bottlerocket-os/twoliter/pull/533
[#535]: https://github.com/bottlerocket-os/twoliter/pull/535
[#537]: https://github.com/bottlerocket-os/twoliter/pull/537
[#549]: https://github.com/bottlerocket-os/twoliter/pull/549
[#550]: https://github.com/bottlerocket-os/twoliter/pull/550
[#553]: https://github.com/bottlerocket-os/twoliter/pull/553
[#555]: https://github.com/bottlerocket-os/twoliter/pull/555

[0.11.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.10.1...v0.11.0

## [0.10.1] - 2025-05-14

### Fixed
* Fix a bug which could prevent pubsys from copying existing AMIs when a secureboot profile is not present in the workspace ([#534])

### Added
* Add a `--dry-run` mode to `pubsys promote-ssm` ([#529])

[#529]: https://github.com/bottlerocket-os/twoliter/pull/529
[#534]: https://github.com/bottlerocket-os/twoliter/pull/534

[0.10.1]: https://github.com/bottlerocket-os/twoliter/compare/v0.10.0..v0.10.1

## [0.10.0] - 2025-05-06

### Added
* Add `amispec` system for customizing AMI registration parameters ([#521], [#524])

### Changed
* Update rust dependencies ([#521], [#522], [#525])
* Fix bug which could prevent pubsys from publishing SSM parameters in new regions ([#518])
* Add more error context to pubsys output on failure ([#525])

### Build
* Add integration test to verify application-inventory generation ([#522])

[#518]: https://github.com/bottlerocket-os/twoliter/pull/518
[#521]: https://github.com/bottlerocket-os/twoliter/pull/521
[#522]: https://github.com/bottlerocket-os/twoliter/pull/522
[#524]: https://github.com/bottlerocket-os/twoliter/pull/524
[#525]: https://github.com/bottlerocket-os/twoliter/pull/525

[0.10.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.9.0..v0.10.0

## [0.9.0] - 2025-04-16

### Added
* Add `external-kmod-development` image feature to control when to build kmod kits ([#512])
* Set `VENDOR_NAME` in `/etc/os-release` of built variants to "Bottlerocket" ([#514])

### Changed
* Update dependencies and migrate to terminal_size ([#517])

### Fixed
* Fix an issue where the `bottlerocket-` prefix is not removed from packages names in `application-inventory.json` when the SDK uses dnf5 ([#515])

[#512]: https://github.com/bottlerocket-os/twoliter/pull/512
[#514]: https://github.com/bottlerocket-os/twoliter/pull/514
[#515]: https://github.com/bottlerocket-os/twoliter/pull/515
[#517]: https://github.com/bottlerocket-os/twoliter/pull/517

[0.9.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.8.1..v0.9.0

## [0.8.1] - 2025-03-13

### Added
* TestSys: Add `assume_role` field to VSphereK8sClusterConfig ([#479])
* TestSys: Support custom block mappings for EC2 instance launch ([#484])

### Changed
* Improve cargo vendor configuration to prevent conflicts with host cargo settings ([#487])
* Acquire a file lock to prevent parallel builds from clashing ([#487])

[#479]: https://github.com/bottlerocket-os/twoliter/pull/479
[#484]: https://github.com/bottlerocket-os/twoliter/pull/484
[#487]: https://github.com/bottlerocket-os/twoliter/pull/487

[0.8.1]: https://github.com/bottlerocket-os/twoliter/compare/v0.8.0...v0.8.1

## [0.8.0] - 2025-03-10

### Changed

- Add dry run mode to pubsys ssm command ([#469])
- Transition to cargo vendor for better cross rust version compatibility ([#470])
- Update rust nightly and update cargo-deny ([#471])
- Add minutes option to parse-datetime for wave definitions ([#473])

### Fixed

- DNF5 compatibility in rpm2img [#457]
- Fix migration and shell checks [#474]
- Prevent SSM parameter promotion from partial parameters updates [#476]
- Aligned GPU flag behavior across K8s and ECS workloads [#480]

[#457]: https://github.com/bottlerocket-os/twoliter/pull/457
[#469]: https://github.com/bottlerocket-os/twoliter/pull/469
[#470]: https://github.com/bottlerocket-os/twoliter/pull/470
[#471]: https://github.com/bottlerocket-os/twoliter/pull/471
[#473]: https://github.com/bottlerocket-os/twoliter/pull/473
[#474]: https://github.com/bottlerocket-os/twoliter/pull/474
[#476]: https://github.com/bottlerocket-os/twoliter/pull/476
[#480]: https://github.com/bottlerocket-os/twoliter/pull/480

[0.8.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.7.3...v0.8.0

## [0.7.3] - 2025-02-21

### Changed

- Improve upstream source fallback logging ([#454])
- Use a consistent length for project build ID ([#456])
- Update kit metadata schema version to v3 due to changes in advisory generation ([#459], [#461])

[#454]: https://github.com/bottlerocket-os/twoliter/pull/454
[#456]: https://github.com/bottlerocket-os/twoliter/pull/456
[#459]: https://github.com/bottlerocket-os/twoliter/pull/459
[#461]: https://github.com/bottlerocket-os/twoliter/pull/461

[0.7.3]: https://github.com/bottlerocket-os/twoliter/compare/v0.7.1...v0.7.3

## [0.7.2] - 2025-02-05

### Fixed

- Fix early exit with docker not on path ([#446])

### Changed

- Update default variant to aws-k8s-1.32 ([#447])
- Update testsys to v0.0.15 ([#448])

[#446]: https://github.com/bottlerocket-os/twoliter/pull/446
[#447]: https://github.com/bottlerocket-os/twoliter/pull/447
[#448]: https://github.com/bottlerocket-os/twoliter/pull/448

[0.7.2]: https://github.com/bottlerocket-os/twoliter/compare/v0.7.1...v0.7.2

## [0.7.1] - 2025-01-23

### Fixed

- Move Docker version check to buildsys ([#442])
- Allow the AWS SDK to find an overridden CA bundle ([#443])

[#442]: https://github.com/bottlerocket-os/twoliter/pull/442
[#443]: https://github.com/bottlerocket-os/twoliter/pull/443

[0.7.1]: https://github.com/bottlerocket-os/twoliter/compare/v0.7.0...v0.7.1

## [0.7.0] - 2025-01-03

### Changed

- Require Docker 23 on build host ([#423])
- Stop requiring a dockerfile syntax image at build time ([#423])

### Fixed

- Stop dereferencing symlinks when traversing project directory ([#431])
- Drop unnecessary `--all` flag from dnf, allowing builds using dnf5 ([#435])

[#423]: https://github.com/bottlerocket-os/twoliter/pull/423
[#431]: https://github.com/bottlerocket-os/twoliter/pull/431
[#435]: https://github.com/bottlerocket-os/twoliter/pull/435

[0.7.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.6.0...v0.7.0

## [0.6.0] - 2024-12-05

### Changed

- Allow arbitrary kits to generate an accurate application inventory ([#410])
- Use `krane` to fetch the SDK during the build instead of `docker` ([#411], [#412])
- Enable verbose `krane` logs when the log level is DEBUG or TRACE ([#411])
- Update `ecr-login` to v0.9.0 ([#411])

[#410]: https://github.com/bottlerocket-os/twoliter/pull/410
[#411]: https://github.com/bottlerocket-os/twoliter/pull/411
[#412]: https://github.com/bottlerocket-os/twoliter/pull/412

[0.6.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.5.1...v0.6.0

## [0.5.1] - 2024-11-11

### Fixed

- Allow projects to not have a `sources/` dir ([#404])
- Write krane to a tempfile instead of a sealed anonymous file ([#405])

[#404]: https://github.com/bottlerocket-os/twoliter/pull/404
[#405]: https://github.com/bottlerocket-os/twoliter/pull/405

[0.5.1]: https://github.com/bottlerocket-os/twoliter/compare/v0.5.0...v0.5.1

## [0.5.0] - 2024-10-10

### Added

- Use bundled `krane` for OCI repository support ([#387])

### Changed

- Increment kit metadata version. This makes this version of Twoliter incompatible with kits built
  from older versions of Twoliter, and older versions of Twoliter incompatible with kits built from
  this version. ([#387])
- Unconditionally use an RPMs NEVR in a variant's application inventory ([#384])

### Fixed

- Allow `find-debuginfo` to manage its own PATH ([#383])
- Refrain from defining RPM macros twice ([#392])

### Removed

- Remove the ability to use `docker` or `crane` from system PATH ([#387])

[#383]: https://github.com/bottlerocket-os/twoliter/pull/383
[#384]: https://github.com/bottlerocket-os/twoliter/pull/384
[#387]: https://github.com/bottlerocket-os/twoliter/pull/387
[#392]: https://github.com/bottlerocket-os/twoliter/pull/392

[0.5.0]: https://github.com/bottlerocket-os/twoliter/compare/v0.4.7...v0.5.0

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

[0.4.7]: https://github.com/bottlerocket-os/twoliter/compare/v0.4.6...v0.4.7

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
