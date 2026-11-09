# AGENTS.md

Development guide for agents working on this codebase.

## Validating Changes

```bash
cargo fmt && make fast   # Quick validation: lint + unit tests
make check  # Full CI (slow): lint + deny + attributions + unit tests + integration tests
```

Do NOT:
- Run cargo commands directly (use make targets)
- Skip `make fast` before committing

## Project Structure

### Main CLI
- `twoliter/` - Main CLI entry point. Orchestrates builds, embeds tool binaries, manages project configuration and SDK interactions.

### Core Tools (`tools/`)
- `buildsys/` - Build system for packages and variants. Handles cargo/rpm build orchestration.
- `pubsys/` - Publishing system. Publishes AMIs, TUF repos, SSM parameters, Kits, etc.
- `pipesys/` - Efficiently share content with docker builds via UDS

### Configuration Crates (`tools/*-config`)
- `buildsys-config/` - Build system configuration types
- `pubsys-config/` - Publishing system configuration

### Utilities (`tools/`)
- `amispec/` - AMI properties specification system
- `bottlerocket-variant/` - Variant type/model definitions
- `error-utils/` - AWS SDK error handling helpers
- `oci-cli-wrapper/` - OCI CLI abstraction layer for container registry operations
- `parse-datetime/` - DateTime parsing utilities
- `serde-templated/` - Supports deserializing structures where each field may be a template to render
- `unplug/` - Used to disable network access during build
- `update-metadata/` - Update metadata serialization (needed for pubsys)

### Embedded Tool Wrappers (`twoliter/embedded/twoliter-tool-*`)
Compressed binaries embedded in the twoliter CLI

### Tests
- `tests/integration-tests/` - Integration test suite (slow, run via `make integ`)

## Code Style

### Commits
Use conventional commits with 52/72 line lengths:
```
feat(buildsys): add new build target support

This commit adds support for building custom targets by extending
the build configuration options.
```

### Design Documentation
See `docs/design/README.md` for architecture details.
