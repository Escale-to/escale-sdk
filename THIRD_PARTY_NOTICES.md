# Third-Party License Review

Last reviewed: 2026-06-22

This file records the third-party Rust crate license review for the Escale SDK.
It is maintained manually and is updated on request, not automatically whenever
code or dependencies change.

The Escale SDK itself is licensed under Apache-2.0.

## Scope

The review intentionally keeps the broader analysis performed on 2026-06-22:

- the crates actually used by the workspace normal dependency graph;
- the broader Cargo package resolution from `Cargo.lock`, including packages
  that may only appear through target-specific, optional, or configuration-gated
  resolution paths.

The normal dependency graph is the primary basis for runtime/build usage. The
broader Cargo resolution is kept here as a conservative compliance snapshot.

## Commands Used

```sh
cargo tree --locked --edges normal --prefix none --format '{p}\t{l}' | sort -u
cargo metadata --locked --format-version 1 --quiet \
  | jq -r '.packages[] | select(.source != null) | [.name, .version, (.license // "NOASSERTION")] | @tsv' \
  | sort -u
cargo check -p epc-image --features jxl-encode-libjxl
```

## Review Result

No incompatible third-party crate license was found for an Apache-2.0 SDK.

No GPL, AGPL, LGPL, MPL, CDDL, proprietary, source-available, or
non-commercial license was identified in the reviewed Cargo dependency data.

The licenses encountered are permissive or Apache-compatible in this context:

- Apache-2.0
- MIT
- BSD-3-Clause
- BSD-1-Clause
- 0BSD
- Unlicense
- Zlib
- Unicode-3.0
- Apache-2.0 WITH LLVM-exception

The Apache Software Foundation third-party license policy classifies licenses
such as Apache-2.0, BSD without advertising clause, MIT/X11, zlib/libpng,
Unicode, Unlicense, and 0BSD as Category A licenses that may be included in
Apache projects, subject to preserving applicable notices and attribution.

Reference: https://www.apache.org/legal/resolved.html

This file is a technical compliance aid, not legal advice.

## Notable Licenses

| Crate(s) | License | Notes |
| --- | --- | --- |
| `ed25519-dalek`, `curve25519-dalek`, `subtle` | BSD-3-Clause | Compatible with Apache-2.0; preserve upstream copyright and license notices. |
| `generic-array`, `simd-adler32`, `zip`, `zmij` | MIT | Compatible with Apache-2.0; preserve copyright and license notices. |
| `jxl-oxide` and `jxl-*` crates | MIT OR Apache-2.0 | Pure Rust JPEG XL decoder stack; compatible with Apache-2.0. |
| `alloc-no-stdlib`, `alloc-stdlib`, `brotli-decompressor` | BSD-3-Clause or BSD-3-Clause/MIT | Brotli decompression dependencies used by the JPEG XL decoder stack; preserve upstream notices. |
| `png`, `bitflags`, `fdeflate` | MIT OR Apache-2.0 or MIT OR Apache-2.0 OR Zlib | Optional `jxl-encode-libjxl` feature dependencies for staging RGBA8 as PNG before invoking `cjxl`. |
| `memchr` | Unlicense OR MIT | Prefer treating as MIT for conservative notice handling. |
| `miniz_oxide` | MIT OR Zlib OR Apache-2.0 | Compatible with Apache-2.0. |
| `bytemuck` | Zlib OR Apache-2.0 OR MIT | Compatible with Apache-2.0. |
| `unicode-ident` | (MIT OR Apache-2.0) AND Unicode-3.0 | Compatible with Apache-2.0; preserve Unicode license notice where required. |
| `adler2` | 0BSD OR MIT OR Apache-2.0 | Compatible with Apache-2.0. |
| `wasi` | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Compatible with Apache-2.0. |
| `fiat-crypto` | MIT OR Apache-2.0 OR BSD-1-Clause | Compatible with Apache-2.0. |

## Normal Dependency Graph

These are the external crates observed in the normal workspace dependency graph
with `cargo tree --locked --edges normal`.

| Crate | Version | License |
| --- | ---: | --- |
| `adler2` | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| `alloc-no-stdlib` | 2.0.4 | BSD-3-Clause |
| `alloc-stdlib` | 0.2.4 | BSD-3-Clause |
| `base64` | 0.22.1 | MIT OR Apache-2.0 |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 |
| `brotli-decompressor` | 5.0.3 | BSD-3-Clause/MIT |
| `bumpalo` | 3.20.3 | MIT OR Apache-2.0 |
| `bytemuck` | 1.25.0 | Zlib OR Apache-2.0 OR MIT |
| `cfg-if` | 1.0.4 | MIT OR Apache-2.0 |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 |
| `crc32fast` | 1.5.0 | MIT OR Apache-2.0 |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 |
| `curve25519-dalek` | 4.1.3 | BSD-3-Clause |
| `curve25519-dalek-derive` | 0.1.1 | MIT/Apache-2.0 |
| `digest` | 0.10.7 | MIT OR Apache-2.0 |
| `displaydoc` | 0.2.6 | MIT OR Apache-2.0 |
| `ed25519` | 2.2.3 | Apache-2.0 OR MIT |
| `ed25519-dalek` | 2.2.0 | BSD-3-Clause |
| `equivalent` | 1.0.2 | Apache-2.0 OR MIT |
| `flate2` | 1.1.9 | MIT OR Apache-2.0 |
| `generic-array` | 0.14.7 | MIT |
| `hashbrown` | 0.17.1 | MIT OR Apache-2.0 |
| `indexmap` | 2.14.0 | Apache-2.0 OR MIT |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 |
| `jxl-bitstream` | 1.1.0 | MIT OR Apache-2.0 |
| `jxl-coding` | 1.0.1 | MIT OR Apache-2.0 |
| `jxl-color` | 0.11.0 | MIT OR Apache-2.0 |
| `jxl-frame` | 0.13.3 | MIT OR Apache-2.0 |
| `jxl-grid` | 0.6.2 | MIT OR Apache-2.0 |
| `jxl-image` | 0.13.0 | MIT OR Apache-2.0 |
| `jxl-jbr` | 0.2.1 | MIT OR Apache-2.0 |
| `jxl-modular` | 0.11.3 | MIT OR Apache-2.0 |
| `jxl-oxide` | 0.12.6 | MIT OR Apache-2.0 |
| `jxl-oxide-common` | 1.0.0 | MIT OR Apache-2.0 |
| `jxl-render` | 0.12.4 | MIT OR Apache-2.0 |
| `jxl-threadpool` | 1.0.0 | MIT OR Apache-2.0 |
| `jxl-vardct` | 0.11.1 | MIT OR Apache-2.0 |
| `log` | 0.4.33 | MIT OR Apache-2.0 |
| `memchr` | 2.8.2 | Unlicense OR MIT |
| `miniz_oxide` | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 |
| `pin-project-lite` | 0.2.17 | Apache-2.0 OR MIT |
| `proc-macro2` | 1.0.106 | MIT OR Apache-2.0 |
| `quote` | 1.0.45 | MIT OR Apache-2.0 |
| `serde` | 1.0.228 | MIT OR Apache-2.0 |
| `serde_core` | 1.0.228 | MIT OR Apache-2.0 |
| `serde_derive` | 1.0.228 | MIT OR Apache-2.0 |
| `serde_json` | 1.0.150 | MIT OR Apache-2.0 |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 |
| `signature` | 2.2.0 | Apache-2.0 OR MIT |
| `simd-adler32` | 0.3.9 | MIT |
| `subtle` | 2.6.1 | BSD-3-Clause |
| `syn` | 2.0.118 | MIT OR Apache-2.0 |
| `thiserror` | 2.0.18 | MIT OR Apache-2.0 |
| `thiserror-impl` | 2.0.18 | MIT OR Apache-2.0 |
| `tracing` | 0.1.44 | MIT |
| `tracing-core` | 0.1.36 | MIT |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 |
| `unicode-ident` | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| `zeroize` | 1.9.0 | Apache-2.0 OR MIT |
| `zip` | 2.4.2 | MIT |
| `zmij` | 1.0.21 | MIT |
| `zopfli` | 0.8.3 | Apache-2.0 |

## Broader Cargo Resolution

These are all external registry packages present in the locked Cargo resolution
at review time.

| Crate | Version | License |
| --- | ---: | --- |
| `adler2` | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| `alloc-no-stdlib` | 2.0.4 | BSD-3-Clause |
| `alloc-stdlib` | 0.2.4 | BSD-3-Clause |
| `arbitrary` | 1.4.2 | MIT OR Apache-2.0 |
| `base64` | 0.22.1 | MIT OR Apache-2.0 |
| `bitflags` | 1.3.2 | MIT OR Apache-2.0 |
| `base64ct` | 1.8.3 | Apache-2.0 OR MIT |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 |
| `brotli-decompressor` | 5.0.3 | BSD-3-Clause/MIT |
| `bumpalo` | 3.20.3 | MIT OR Apache-2.0 |
| `bytemuck` | 1.25.0 | Zlib OR Apache-2.0 OR MIT |
| `cfg-if` | 1.0.4 | MIT OR Apache-2.0 |
| `const-oid` | 0.9.6 | Apache-2.0 OR MIT |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 |
| `crc32fast` | 1.5.0 | MIT OR Apache-2.0 |
| `crossbeam-utils` | 0.8.21 | MIT OR Apache-2.0 |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 |
| `curve25519-dalek` | 4.1.3 | BSD-3-Clause |
| `curve25519-dalek-derive` | 0.1.1 | MIT/Apache-2.0 |
| `der` | 0.7.10 | Apache-2.0 OR MIT |
| `derive_arbitrary` | 1.4.2 | MIT OR Apache-2.0 |
| `digest` | 0.10.7 | MIT OR Apache-2.0 |
| `displaydoc` | 0.2.6 | MIT OR Apache-2.0 |
| `ed25519` | 2.2.3 | Apache-2.0 OR MIT |
| `ed25519-dalek` | 2.2.0 | BSD-3-Clause |
| `equivalent` | 1.0.2 | Apache-2.0 OR MIT |
| `fiat-crypto` | 0.2.9 | MIT OR Apache-2.0 OR BSD-1-Clause |
| `fdeflate` | 0.3.7 | MIT OR Apache-2.0 OR Zlib |
| `flate2` | 1.1.9 | MIT OR Apache-2.0 |
| `generic-array` | 0.14.7 | MIT |
| `getrandom` | 0.2.17 | MIT OR Apache-2.0 |
| `hashbrown` | 0.17.1 | MIT OR Apache-2.0 |
| `indexmap` | 2.14.0 | Apache-2.0 OR MIT |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 |
| `jxl-bitstream` | 1.1.0 | MIT OR Apache-2.0 |
| `jxl-coding` | 1.0.1 | MIT OR Apache-2.0 |
| `jxl-color` | 0.11.0 | MIT OR Apache-2.0 |
| `jxl-frame` | 0.13.3 | MIT OR Apache-2.0 |
| `jxl-grid` | 0.6.2 | MIT OR Apache-2.0 |
| `jxl-image` | 0.13.0 | MIT OR Apache-2.0 |
| `jxl-jbr` | 0.2.1 | MIT OR Apache-2.0 |
| `jxl-modular` | 0.11.3 | MIT OR Apache-2.0 |
| `jxl-oxide` | 0.12.6 | MIT OR Apache-2.0 |
| `jxl-oxide-common` | 1.0.0 | MIT OR Apache-2.0 |
| `jxl-render` | 0.12.4 | MIT OR Apache-2.0 |
| `jxl-threadpool` | 1.0.0 | MIT OR Apache-2.0 |
| `jxl-vardct` | 0.11.1 | MIT OR Apache-2.0 |
| `libc` | 0.2.186 | MIT OR Apache-2.0 |
| `log` | 0.4.33 | MIT OR Apache-2.0 |
| `memchr` | 2.8.2 | Unlicense OR MIT |
| `miniz_oxide` | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 |
| `pin-project-lite` | 0.2.17 | Apache-2.0 OR MIT |
| `pkcs8` | 0.10.2 | Apache-2.0 OR MIT |
| `png` | 0.17.16 | MIT OR Apache-2.0 |
| `proc-macro2` | 1.0.106 | MIT OR Apache-2.0 |
| `quote` | 1.0.45 | MIT OR Apache-2.0 |
| `rand_core` | 0.6.4 | MIT OR Apache-2.0 |
| `rustc_version` | 0.4.1 | MIT OR Apache-2.0 |
| `semver` | 1.0.28 | MIT OR Apache-2.0 |
| `serde` | 1.0.228 | MIT OR Apache-2.0 |
| `serde_core` | 1.0.228 | MIT OR Apache-2.0 |
| `serde_derive` | 1.0.228 | MIT OR Apache-2.0 |
| `serde_json` | 1.0.150 | MIT OR Apache-2.0 |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 |
| `signature` | 2.2.0 | Apache-2.0 OR MIT |
| `simd-adler32` | 0.3.9 | MIT |
| `spki` | 0.7.3 | Apache-2.0 OR MIT |
| `subtle` | 2.6.1 | BSD-3-Clause |
| `syn` | 2.0.118 | MIT OR Apache-2.0 |
| `thiserror` | 2.0.18 | MIT OR Apache-2.0 |
| `thiserror-impl` | 2.0.18 | MIT OR Apache-2.0 |
| `tracing` | 0.1.44 | MIT |
| `tracing-core` | 0.1.36 | MIT |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 |
| `unicode-ident` | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| `version_check` | 0.9.5 | MIT/Apache-2.0 |
| `wasi` | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `zeroize` | 1.9.0 | Apache-2.0 OR MIT |
| `zip` | 2.4.2 | MIT |
| `zmij` | 1.0.21 | MIT |
| `zopfli` | 0.8.3 | Apache-2.0 |

## Maintenance Notes

When this file is updated, re-run the commands above from the `escale-sdk`
directory and review any new license expression before changing this file.

For binary or source distributions, preserve required third-party license and
copyright notices according to the upstream license terms.
