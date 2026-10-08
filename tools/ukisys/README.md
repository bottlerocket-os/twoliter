# ukisys

Command-line construction and PE section removal for Bottlerocket Unified Kernel Images (UKIs).

`cmdline` combines binary bootconfig with base arguments and writes a file for
`ukify --cmdline @FILE`. Kernel parameters precede base kernel arguments; init
parameters follow `--`, before base init arguments.

`derive-stub` recovers the unsigned systemd-stub a signed UKI was built from: it strips the Authenticode signature and truncates the trailing `.osrel`/`.cmdline`/`.uname`/`.linux` sections, patching `NumberOfSections`, `SizeOfInitializedData`, and `SizeOfImage`, and zeroing `CheckSum`.

All four trailing sections are removed together, not just the three payload sections, because `ukify build` overwrites any same-named section already present in the stub in place rather than skipping it.

## Usage

```bash
ukisys cmdline --bootconfig bootconfig.data \
  --kernel-args 'console=ttyS0 root=/dev/dm-0' \
  --init-args 'systemd.show_status=true' --output cmdline
ukisys derive-stub <input-uki> <output-stub>
```
