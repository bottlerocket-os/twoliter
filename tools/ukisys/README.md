# ukisys

Native PE section-removal tool for Bottlerocket Unified Kernel Images (UKIs).

`derive-stub` recovers the unsigned systemd-stub a signed UKI was built from: it strips the Authenticode signature and truncates the trailing `.osrel`/`.cmdline`/`.uname`/`.linux` sections, patching `NumberOfSections`, `SizeOfInitializedData`, and `SizeOfImage`, and zeroing `CheckSum`.

All four trailing sections are removed together, not just the three payload sections, because `ukify build` overwrites any same-named section already present in the stub in place rather than skipping it.

## Usage

```bash
ukisys derive-stub <input-uki> <output-stub>
```
