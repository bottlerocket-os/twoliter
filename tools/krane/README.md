## krane-bundle

This crate packages the `krane` utility from [google/go-containerregistry].

The utility is downloaded as an **official prebuilt release binary** (pinned to
a specific tag) by the build script, checksum-verified, then compressed and
embedded into the crate via `include_bytes!`.

The release version is set by `CRANE_VERSION` in `build.rs`. The prebuilt
tarball is selected from the current build target: the archive names follow
goreleaser's `Linux_x86_64`, `Linux_arm64` naming from
<https://github.com/google/go-containerregistry/releases>.

At runtime, `krane-bundle` writes the decompressed binary to a temp file,
passing the filepath of that file to any caller.

### Bumping the pinned version

Builds only pull archives from `cache.bottlerocket.aws`, keyed by SHA512. Any
new version must be **uploaded to the cache before** the version bump PR lands
so that CI (which does *not* enable the upstream fallback) can fetch it.

Steps:

1. Update `CRANE_VERSION` in `build.rs`.
2. Regenerate the SHA512 sums under `hashes/` for each supported target
   (`Linux_x86_64`, `Linux_arm64`, `source`).
3. Update the URL comment at the top of each hash file to point at the new
   release.
4. Mirror the archives into the lookaside cache that backs
   `cache.bottlerocket.aws` under the path `build-cache-fetch` requests:
   `<filename>/<sha512>/<filename>`.

After uploading, confirm CI will be happy by fetching each pinned hash
without the fallback:

```console
for f in tools/krane/hashes/*; do
  (cd "$(mktemp -d)" && UPSTREAM_SOURCE_FALLBACK=false \
     "${OLDPWD}/tools/build-cache-fetch" "${OLDPWD}/${f}")
done
```

### Building locally against an un-mirrored version

For a first-time local build against a version that is not yet mirrored, set
`UPSTREAM_SOURCE_FALLBACK=true` when running `cargo build`. The upstream
GitHub Releases URL is still SHA512-verified against the pinned hash file, so
this only affects *availability* (where the bytes come from), not integrity.
Do not enable the fallback in CI: the whole point of the cache-only model is
that a compromised or moved upstream release cannot silently affect builds.

[google/go-containerregistry]: https://github.com/google/go-containerregistry
