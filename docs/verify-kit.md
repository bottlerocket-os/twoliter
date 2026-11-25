# Verifying a kit

`twoliter verify kit bottlerocket-core-kit 17.3.0` checks that a published kit can be reproduced
from source:

1. It reads the kit image index `<registry>/<kit>:v<version>`, where `--registry` defaults to
   `public.ecr.aws/bottlerocket`, and downloads the image of each architecture to verify
   (`--arch`, repeatable; defaults to every architecture in the index, and fails if the index
   lists one that cannot be verified, or more than one manifest for an architecture). The kit
   metadata in the image config must name the requested kit and version.
2. It clones the source (`--repo`, default `https://github.com/bottlerocket-os/<kit>`) and checks
   out the commit tagged `v<version>`, or the one given with `--commit`, removing every untracked
   and ignored file. An existing clone given with `--source-dir` must have no uncommitted changes
   and no untracked or ignored files, such as the output of an earlier build. The commit's release
   version must be the kit's version.
3. It warns when the source pins a twoliter older than 0.28.0, the first whose kit builds are
   expected to reproduce: older ones order the kit's layers by digest and write repository
   metadata that depends on the build time.
4. It runs `make build KIT=<kit> ARCH=<arch>` in the checkout for each architecture, so the kit
   is rebuilt with the twoliter version pinned in its Makefile. The build output is written to
   `build-<arch>.log` in the work directory, and its last lines are shown if the build fails;
   `--verbose` shows it as it runs instead.

   The build uses the SDK for `--sdk-arch` (default x86_64, which Bottlerocket kits are built
   on), since each SDK architecture's toolchains produce different code. Other host architectures
   need QEMU binfmt (e.g. `docker run --privileged --rm tonistiigi/binfmt --install amd64`) and
   are slower.

   It then unpacks the package layers of each published image and regenerates their repository
   metadata in the SDK, without network access, as rpm2kit does: `createrepo_c --revision <commit
   time> --set-timestamp-to-revision`.
5. It compares each rebuilt image against the published one with `reprosys compare-kit`. Every
   blob of both images must match the digest that names it. An architecture is verified when its
   rebuilt manifest digest equals the published one, which makes the config and every layer
   identical. Otherwise:
   * The configs must be equal apart from `rootfs.diff_ids`, and the published `diff_ids` must be
     its manifest's layer digests, in order (kit layers are uncompressed).
   * Layers are matched by the package directory or `repodata` they hold. Within a differing
     package layer, every entry must have the same type, mode, ownership, modification time and
     link target, and each package whose bytes differ is compared as below.
   * A differing `repodata` layer must hold exactly the regenerated metadata, file for file and
     byte for byte, with the same tar attributes as the rebuilt layer. This covers `repomd.xml`,
     `primary`, `filelists`, `other` and the dependency data.
   * Anything else in the manifests, such as layer order or annotations, must be equal.

   The summary lists what differs, as rebuilt vs published, and the full JSON report is kept in
   the work directory (`--work-dir`, default `./twoliter-verify-<kit>-v<version>`). `--verbose`
   lists expected differences, which are otherwise counted.
6. Each package whose bytes differ is compared by header and payload:
   * Both payloads must be zstd-compressed cpio archives. They are walked in lockstep: every entry
     must have the same name, mode, owner, group, link count, inode, modification time and device
     numbers, in the same order, and both archives must end at the same entry, followed by the same
     bytes. Contents must be identical, apart from these expected differences (`note`):
     * Go 1.25 to 1.27 give cgo packages compiled with link-time optimization a random action ID
       on every build ([golang/go#81040](https://github.com/golang/go/issues/81040), fixed in
       Go 1.28). The Bottlerocket SDK's `CGO_CFLAGS` enable LTO, so the build IDs of Go binaries
       using cgo differ even though their code is identical. A Go binary is an expected difference
       when only the action IDs of its Go build ID differ, its content IDs are equal, and every
       byte outside its Go and GNU build ID notes is identical.
     * Each kernel build generates its own module signing key. A kernel module (`.ko`, or
       `.ko.gz` holding one gzip member) is an expected difference when it differs only in its
       appended signature.
     * The kernel image `/boot/vmlinuz` embeds the build's signing certificate inside its
       compressed kernel. It is unwrapped (an x86_64 bzImage, or an aarch64 EFI zboot image) and
       its zstd or gzip payload decompressed, with nothing after it but the decompressed size. The
       decompressed kernels must be the same length and identical, apart from the x86_64 GNU build
       ID and one window of at most 4 KiB, where the certificate is.
   * Header tags must be equal, except those derived from the payload, which are recomputed from
     the published payload and must equal what the published header records: the SHA-256 digest
     and size of each differing file, the installed size, and the payload digests and sizes. The
     signature header must have the same tags as the rebuilt one, and its SHA-1 and SHA-256
     header digests must match the published header; its other values are not compared.

   Kits are expected to reproduce byte for byte apart from these differences, including
   `bottlerocket-kernel-kit`.

Not checked:

* The kernel's boot wrapper: the bzImage setup code, the EFI stub and the decompressor. They hold
  the compressed kernel, which differs with the certificate, so they cannot be compared byte for
  byte.
* The bytes within the kernel image's certificate window: the certificate itself, and anything
  else in the same 4 KiB.
* Module signatures: they are stripped by the length they record. The kernel checks them when it
  loads a module.
* `FILECLASS` and `CLASSDICT`, the file types libmagic reads from the contents, which include ELF
  build IDs.
* The signature header's values other than the header digests, such as its sizes and the
  legacy MD5 digest of header and payload.

A long-lived module signing key in the kernel kit would make the kernel image and modules
reproduce exactly.

The command exits non-zero unless every architecture verifies. Requirements: Docker, git, make,
and disk space for the build plus the downloaded kit images.
