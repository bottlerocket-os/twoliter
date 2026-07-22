# eif-builder

Builds a minimal sidecar [EIF (Enclave Image Format)][eif] for AWS Nitro
Enclaves: kernel + cmdline + empty ramdisk + metadata. The rootfs is *not*
embedded; it is a separate erofs artifact attached as virtio-blk at launch.

[eif]: https://github.com/aws/aws-nitro-enclaves-image-format

## Kernel image format

The `--kernel` input format is architecture-dependent:

- **x86_64**: an uncompressed ELF `vmlinux` (Firecracker PVH boot). No
  pre-processing is applied; the input bytes are embedded verbatim.
- **aarch64**: either a flat PE-wrapped arm64 kernel `Image` (arm64 Linux boot
  protocol, `MZ` + `ARM\x64` at offset 56), or an EFI zboot outer image
  wrapping one. zboot images are unwrapped and decompressed transparently
  (gzip and zstd are supported) so the EIF always embeds a bootable flat
  `Image`.

Recent Bottlerocket arm64 kernel-kit RPMs ship `vmlinuz` as an EFI zboot
image, which is why the aarch64 path exists. Firecracker's arm64 loader does
not know how to unwrap zboot, so `eif-builder` does it before writing the
kernel section.

## Target architecture

The EIF header's architecture flag identifies the *target* arch of the
kernel embedded in the `EifSectionKernel` section, not the arch of the
machine building the EIF. Cross-arch builds are common (e.g. an aarch64
builder producing an x86_64 EIF, or vice versa), so callers must pass the
target arch explicitly via `--arch`; it is never inferred from the build
host.

Accepted values: `x86_64`/`amd64`, `aarch64`/`arm64`.

## Ramdisk

The EIF's ramdisk section is emitted as a well-formed but empty section
(12-byte header, zero payload). Bottlerocket sidecar EIFs mount the rootfs
from a virtio-blk device with dm-verity, so no initramfs is needed.

Note that stock `nitro-cli`-produced EIFs always have two ramdisks
(bootstrap + customer). Consumers that assume that convention (e.g. anything
computing PCR2 over concatenated ramdisks) will see empty input for EIFs
produced by this tool. The Bottlerocket launcher handles this correctly.

## Metadata and attestation

The metadata section is a JSON object matching the schema expected by the NE
hypervisor (top-level keys and `BuildMetadata` sub-keys mirror
`aws-nitro-enclaves-image-format`). PCR/measurement fields produced by the
upstream AWS Nitro Enclaves CLI are intentionally omitted.

The EIF itself is measured into PCRs by the hypervisor at launch, so its
kernel, cmdline, and (dm-verity-anchored) rootfs handoff are covered by
attestation. Bespoke rootfs measurement is out of scope for this tool.

## `--out-prepared-kernel`

The optional `--out-prepared-kernel PATH` flag writes the *prepared* kernel
bytes (the exact byte stream embedded in the EIF's kernel section) to a
separate file. On aarch64 this is the unwrapped/decompressed flat PE
`Image`; on x86_64 it is the input verbatim.

This is intended for local dev smoke-tests where a plain Firecracker (no NE
fork) needs a kernel image its PE loader will accept. The EIF is written
unchanged whether or not the flag is set.
