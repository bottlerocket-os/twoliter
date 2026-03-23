# Custom Variant Names and Variant Attribute Overrides

### Background

Bottlerocket variants can override attributes like `platform` and `runtime` to inherit build-time behavior from standard variant families. For example, a custom variant named `foo-k8s-1.35` might override:

```toml
[package.metadata.build-variant]
platform = "vmware"
runtime = "k8s"
```

This results in `family = aws-k8s`, which affects build-time behavior such as:
- Conditional compilation via `cfg` attributes
- AMI naming conventions
- Platform-specific package inclusion

However, these overrides do **not** automatically configure the settings-defaults RPM resolution.

## The Settings-Defaults Problem

The `settings-defaults` RPM package uses exact variant name matching via RPM dependencies. When the variant build runs:

1. The `bottlerocket-metadata` RPM provides `variant(NAME)` based on your literal variant name (e.g., `variant(foo-k8s-1.35)`)
2. The RPM resolver looks for a `settings-defaults` subpackage with a matching `Requires: variant(foo-k8s-1.35)`
3. If no match exists, the build fails with: `Unable to find a match: bottlerocket-settings-defaults`

Variant attribute overrides affect build-time behavior but do **not** affect RPM dependency resolution.

## Required Steps for Custom Variants

### 1. Create a Settings-Defaults Directory

You must create a settings-defaults directory named after your **actual variant name**, not the family you're inheriting from.

In the Bottlerocket repository under `sources/settings-defaults/`, create:

```
foo-k8s-1.35/
├── Cargo.toml
└── defaults.d/
    ├── 10-defaults.toml → ../../shared-defaults/defaults.toml
    ├── 15-aws-tuf.toml → ../../shared-defaults/aws-tuf.toml
    ├── 20-aws-host-containers.toml → ../../shared-defaults/aws-host-containers.toml
    └── 50-kubernetes-aws.toml → ../../shared-defaults/kubernetes-aws.toml
```

The `Cargo.toml` should define the crate name using underscores (dots are not allowed in crate names):

```toml
[package]
name = "settings-defaults-foo-k8s-1_35"
version = "0.1.0"
edition = "2021"
publish = false
build = "../build.rs"

[build-dependencies]
gensettings = { path = "../../gensettings" }
```

### 2. Use Symlinks to Inherit Shared Defaults

The `defaults.d/` directory should contain numbered symlinks to files in `shared-defaults/`. The numbered prefix controls merge order (lower numbers are applied first).

Choose symlinks based on your target family:

| Family | Required Symlinks |
|--------|------------------|
| aws-k8s | `defaults.toml`, `aws-tuf.toml`, `aws-host-containers.toml`, `kubernetes-aws.toml` |
| vmware-k8s | `defaults.toml`, `public-tuf.toml`, `public-host-containers.toml`, `kubernetes-vmware.toml` |
| metal-k8s | `defaults.toml`, `public-tuf.toml`, `public-host-containers.toml`, `kubernetes-metal.toml` |
| aws-ecs | `defaults.toml`, `aws-tuf.toml`, `aws-host-containers.toml`, `ecs.toml` |

### 3. Update the RPM Spec

Add your variant to `packages/settings-defaults/settings-defaults.spec`. You can either:

**Option A: Create a new subpackage**:

```spec
%package foo-k8s-1.35
Summary: Settings defaults for foo-k8s-1.35 variant
Requires: %{_cross_os}variant(foo-k8s-1.35)
Provides: %{_cross_os}settings-defaults(any)

%description foo-k8s-1.35
%{summary}.

%files foo-k8s-1.35
%{_cross_factorydir}/%{_cross_os}defaults.d/*
```

**Option B: Add to an existing subpackage** (if settings are compatible):

```spec
%package aws-k8s-1.35
...
Requires: (%{shrink:
           %{_cross_os}variant(aws-k8s-1.35)      or
           %{_cross_os}variant(aws-k8s-1.35-fips) or
           %{_cross_os}variant(foo-k8s-1.35) # Add your variant here
           %{nil}})

```
