## `include-env-compressed`

Like `include-bytes!`, but adds `zstd` compression for release builds.

This macro is intended to be used alongside [cargo artifact-dependencies].
As such, the input must be an environment variable which is set at compile time to a path to the
target asset.

Use the macro like so:

```rust
use include_env_compressed: {Archive, include_archive_from_env};

const MY_ARCHIVE: Archive = include_archive_from_env!("ENV_VAR");

// Returns a `Box<dyn std::io::Read + Send + Sync + 'static>`
let reader = MY_ARCHIVE.reader();
```

You may optionally specify a zstd compression level as the second argument.

```rust
const VERY_COMPRESSED_ARCHIVE: Archive = include_archive_from_env!("ENV_VAR", 22);
```

Uses zstd's default compression level if no level is provided.

Note: Due to limitations, does not compose with the `env!()` macro in the same way that
`include_bytes!` does.

[cargo artifact-dependencies]: https://doc.rust-lang.org/beta/cargo/reference/unstable.html#artifact-dependencies
