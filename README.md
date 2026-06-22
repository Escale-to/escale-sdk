# Escale SDK

Reference SDK and command-line tooling for Escale Presence Capsule (EPC).

## Crates

- `epc-core`: shared EPC domain types and core rules.
- `epc-validate`: validation model and reports.
- `epc-pack`: EPC archive assembly.
- `epc-cli`: command-line interface.

## Documentation

Generate local Rust API documentation with:

```sh
cargo doc --no-deps
```

The generated entry point is:

```text
target/doc/escale_epc/index.html
```

## License

Apache-2.0

## How To

Usage:

```sh
cargo run -p epc-cli -- --help
cargo run -p epc-cli -- --version
```

Available commands:

```sh
cargo run -p epc-cli -- validate <capsule.epc>
cargo run -p epc-cli -- validate-dir <unpacked-capsule-dir>
cargo run -p epc-cli -- create [--force] <draft-dir> <author-display-name>
cargo run -p epc-cli -- sign --ssh-key <ssh-ed25519-key> <source-dir>
cargo run -p epc-cli -- pack <source-dir> [output-dir]
cargo run -p epc-cli -- pack --sign <ssh-ed25519-key> <source-dir> [output-dir]
cargo run -p epc-cli -- generate-test-vectors ../escale-design/test-vectors/core-format
```

Capsule creation:

```sh
cargo run -p epc-cli -- create ../my-card "Bruno"
```

This creates:

```sh
my-card/
  manifest.json
  media/
  text/
    message.md
```

Add `media/cover.jxl`, `media/thumbnail.jxl`, and edit `text/message.md` before
packing.

No need to create `proof/hashes.json` because `pack` generates it
automatically before writing the `.epc` file.

`create` initializes an unpacked draft tree. The ADR-003 filename for an
exported or stored draft is:

```text
escale-<ID10>.epc
```

`pack` takes an optional output directory, not an output filename. When omitted,
the output directory defaults to the parent directory of `<source-dir>`. The
sealed capsule filename is generated from ADR-003:

```text
<TIME6>-<ID10>.epc
```

`TIME6` is derived from `sealed_at`; `ID10` is the last 10 characters of the
canonical `escale:<ULID>` id.

When using `create`, `id`, `created_at`, and `created_local_time` are generated
and `sealed_at` is left empty. `created_at` is the canonical UTC creation
instant. `created_local_time` records the creating device's local time context at
that same instant: `time_zone` should be the device time zone identifier
reported by the OS, preferably IANA form such as `Europe/Paris`, and
`utc_offset` is the device's effective offset at creation time. Mobile and
embedded SDK integrations should supply this value from the device API when
building the `CreateDraftRequest`; the reference CLI uses best-effort host
detection.

When using `pack`, `created_at` and `created_local_time` are kept as the
draft/card creation metadata. If `sealed_at` is empty, it is written by the
packer at sealing time, before `proof/hashes.json` and the filename are
generated. If `sealed_at` is already present, `pack` keeps it unchanged so
repeated packs produce the same ADR-003 filename. `pack` refuses to overwrite an
existing `.epc` archive.

Authenticity is added with `sign`, which writes `proof/signature.json`:

```sh
cargo run -p epc-cli -- sign --ssh-key ~/.ssh/id_ed25519_escale ../my-card
```

Or in one step while packing:

```sh
cargo run -p epc-cli -- pack --sign ~/.ssh/id_ed25519_escale ../my-card
```

If `proof/signature.json` already exists, `sign` refuses to overwrite it. Use
`--force` to replace the existing signature proof deliberately:

```sh
cargo run -p epc-cli -- sign --force --ssh-key ~/.ssh/id_ed25519_escale ../my-card
cargo run -p epc-cli -- pack --sign ~/.ssh/id_ed25519_escale --force ../my-card
```

`pack --sign` reuses an existing `proof/signature.json` by default; it signs only
when no signature proof is present. This lets repeated packing remain stable.

`sign` seals the draft when needed, regenerates `proof/hashes.json`, signs the
canonical EPC signature payload with the OpenSSH Ed25519 private key, and records
the public key, key id, policy, and signature value. The signature binds to
`proof/hashes.json.core_digest`; it does not change the immutable core digest.
Encrypted OpenSSH private keys are not supported yet.

If `manifest.json` already exists, `create` refuses to overwrite it. Use
`create --force` to reset the draft manifest; this regenerates `id` and
creation metadata, keeps `sealed_at` empty, and leaves existing message/media
files untouched.

Minimal `manifest.json` file:

```json
{
  "epc_version": "1.0",
  "profile": "core-format",
  "type": "postcard",
  "id": "escale:01J0Y3J7Q9M8W2N6K4R5T8X9AZ",
  "created_at": "2026-06-17T10:00:00Z",
  "created_local_time": {
    "time_zone": "Europe/Paris",
    "utc_offset": "+02:00"
  },
  "sealed_at": "",
  "author": {
    "display_name": "Bruno"
  },
  "content": {
    "cover": {
      "path": "media/cover.jxl",
      "mime": "image/jxl"
    },
    "thumbnail": {
      "path": "media/thumbnail.jxl",
      "mime": "image/jxl"
    },
    "message": {
      "path": "text/message.md",
      "mime": "text/markdown",
      "markdown_profile": "epc-markdown-core",
      "markdown_profile_version": "1.0"
    }
  }
}
```

Example:

```sh
output=$(cargo run -q -p epc-cli -- pack ../my-card)
cargo run -q -p epc-cli -- validate "$output"
```

---
