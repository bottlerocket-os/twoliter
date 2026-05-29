# guest-images-kit

A test fixture exercising the `guest-images` workflow:

- `variants/inner-variant` — A simple guest variant that produces disk images.
- `variants/wrapper-variant` — A host variant that declares `inner-variant` under
  `[package.metadata.build-variant.guest-images]`, so its built images are copied directly
  into the host's rootfs at `/usr/share/bottlerocket/guests/inner` during the host's image
  build.
- `kits/wrapper-kit` — A kit consumed by `wrapper-variant` that bundles ordinary packages.
- `packages/inner-pkg` — A normal package, included in the kit and pulled into the host
  variant via `included-packages`.
