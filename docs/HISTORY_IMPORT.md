# History import file (version 1)

CRFty can import conversion history through a versioned JSON exchange file.
The app knows nothing about any older history format: a standalone converter
script shipped in this repository (`tools/export_history_v3.py`, stdlib-only
Python run against a V2 `conversion_history.json`) reads the old history,
performs all source-format interpretation (path recovery, timestamp parsing,
float scrubbing, status mapping), and emits this schema. The app strictly parses it — a malformed or unknown-version file is
rejected whole, never salvaged record by record.

Reader: `crfty-engine/src/history_import.rs`. Import flow: records land in
`DurableState.parked`, keyed by normalized source path; when a queued file is
prepared, its path spellings are matched against the parked inbox and each
hit is adopted (verdict and provenance onto the observed content record) or
retired (the file no longer matches). The inbox empties itself.

## Container

```json
{
  "import_version": 1,
  "records": [ { …record… } ]
}
```

- `import_version` (required, integer): must be `1`.
- `records` (required, array): zero or more record objects.
- Unknown fields anywhere are version skew and reject the file.

## Record

Only `path` and `status` are required. Every other field is optional and
omitted when the source had nothing honest to say. All numbers are integers —
never floats — and 64-bit values must stay within JSON-safe integer range
except `modified_ns`, which is a decimal string.

| Field | Type | Meaning |
|---|---|---|
| `path` | string, required | Absolute source path as the producing machine knew it. |
| `status` | string, required | `scanned` \| `analyzed` \| `not_worthwhile` \| `converted`. |
| `size` | integer | Byte size of the source file when the record was decided. |
| `modified_ns` | string | Modification time, nanoseconds since Unix epoch, as a decimal string (exceeds JSON-safe integers). |
| `video_codec` | string | `av1` \| `h264` \| `hevc` \| `vp9`; any other value is preserved verbatim. |
| `width`, `height` | integer | Source dimensions in pixels. |
| `duration_ms` | integer | Source duration in milliseconds. |
| `output_size` | integer | Byte size of the conversion output (converted records). |
| `encoding_time_ms` | integer | Wall-clock encoding time in milliseconds. |
| `crf_thousandths` | integer | CRF × 1000 (e.g. CRF 30 → `30000`). |
| `vmaf_hundredths` | integer | Achieved VMAF × 100, at most `10000`. |
| `target` | integer 0–100 | VMAF target the result satisfied. |
| `requested_target` | integer 0–100 | Originally requested VMAF target (not-worthwhile records). |
| `floor_target` | integer 0–100 | Fallback floor that was exhausted (not-worthwhile records). |
| `decided_at_ms` | integer | When the record was decided, milliseconds since Unix epoch. Missing → the import instant. |

## Path matching

Records are keyed by the normalized form of `path`, and the same rule is
applied to a queued file's canonical and absolute spellings at prepare time:

1. strip Windows verbatim prefixes (`\\?\UNC\server\share\…` →
   `\\server\share\…`, `\\?\C:\…` → `C:\…`),
2. replace backslashes with forward slashes,
3. lowercase (ASCII).

Emit paths in the spelling most likely to match how the app will see the
file. On Windows, `std::fs::canonicalize` resolves mapped drive letters to
UNC (`\\server\share\…`), so paths on network shares should be emitted in
UNC form; local paths keep their drive letter. Duplicate keys within one
file are allowed — the reducer keeps the first and counts the rest as
skipped. Records whose files have moved will simply never match and can be
retired from the parked inbox later.

## What deliberately does not cross

- CRF-search caches: analysis identity in v3 is profile-exact (ADR-007), so
  a search from another toolchain can never honestly qualify for reuse.
- Probe caches (audio streams, bitrate), estimates, first-seen timestamps:
  recomputable or valueless.
- Anonymized records: a hashed path can never match a real file; the
  converter reports and drops them.
