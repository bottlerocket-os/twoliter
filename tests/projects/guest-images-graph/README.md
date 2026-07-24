# guest-images-graph

Minimal Cargo workspace used only by buildsys unit tests to exercise the
`guest-images` dependency-graph rules. The shape is:

```
host (variant)
├─ [build-deps] direct (variant)         <- valid guest
└─ [build-deps] middle-kit (kit)
   ├─ [deps]    transitive (variant)     <- reachable but NOT a direct build-dep
   └─ [deps]    some-pkg (package)
```

No crate here is intended to be passed to `twoliter build`; the fixtures only
need to be valid enough for `cargo metadata --offline` to succeed.
