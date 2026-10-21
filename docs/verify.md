# How `twoliter verify` works

`twoliter verify ami` and `twoliter verify kit` answer one question: **was this published artifact
built from the source it claims?** Both rebuild the artifact from that source and compare the result
against what was published, treating the published copy as untrusted input.

## Flow

Both commands share the same shape: resolve → fetch → rebuild → compare → report.

| Step | `verify ami --ami-id <id>` | `verify kit <kit> <version>` |
|---|---|---|
| Resolve | Read variant, arch, version and commit from the AMI name / snapshot description. Reject dirty builds. | Fetch the OCI index for `<registry>/<kit>:v<version>`; read the kit name and version from the image config label; find the commit tagged `v<version>` (or `--commit`). |
| Fetch | Download the root and data snapshots with coldsnap (copying cross-account snapshots first, deleting the copies after). | Pull each architecture's image into an OCI layout. |
| Rebuild | Check out the commit and run `cargo make` so the image is built with the twoliter pinned in the repo. Runs in parallel with the download. | Check out the commit, pull the SDK pinned in `Twoliter.lock` for `--sdk-arch`, and run `make build KIT=<kit> ARCH=<arch>` per architecture. Regenerate the repository metadata of the published packages with the SDK's `createrepo_c`. |
| Compare | `reprosys compare` on the two disk images, region by region. | `reprosys compare-kit`: blob and manifest digests; on mismatch, config fields, layers, each differing RPM by header and payload, the repository metadata against the regenerated one, and the rest of the manifest. |
| Report | Terminal summary (failures first, expected differences counted), `report.json`, `log.jsonl`. Exit non-zero unless `verified`. | Same, without `log.jsonl`. |

## Source checkout

When twoliter clones the source itself (the default, or `--repo`), it checks out the commit with
`--force` and removes every untracked and ignored file with `git clean -ffdx`, so a clone reused
from an earlier run in the work directory cannot carry output of an earlier build into the
rebuild. An existing clone given with `--source-dir` is never cleaned: it must have no
uncommitted changes and no untracked or ignored files (`git status --porcelain --ignored` is
empty), since build output left in it, such as the `build` directory, could change the build or
be taken for its output.

The comparison runs in `reprosys`, a tool embedded in twoliter that reads every format itself
(GPT, ext4, erofs, FAT, dm-verity, RPM headers and cpio payloads, ELF notes, PE sections) from the
raw bytes, without mounting or root.

See [Verifying an AMI](../README.md#verifying-an-ami) and [Verifying a kit](verify-kit.md) for
the options and the rules each comparison applies.
