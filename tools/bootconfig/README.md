# bootconfig

Bootconfig parsing and boot command-line construction shared by `ukisys` and `pcrsys`.

## Usage

Use `parse` for standalone binary bootconfig produced by the SDK tool, or `parse_text` for text.


`uki_cmdline` combines bootconfig and base arguments, separating kernel and init arguments with `--`.
`uki_load_options` produces the UTF-16LE measurement buffer.
`predict_grub_cmdline` handles GRUB variables, boot-partition UUID substitution, and quote repair.

## Input

The parser supports flags, quoted values, arrays, and nested keys in Linux bootconfig order.
Unsupported assignments and invalid binary size, checksum, or padding return errors.
UKI command lines must be printable ASCII with balanced quotes and fit within 2048 bytes including NUL.
See the API documentation for syntax restrictions.
