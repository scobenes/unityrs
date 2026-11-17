# Driving unityrs from agents and scripts

Start with `unityrs describe`, then `unityrs inspect INPUT...`, then
`unityrs show INPUT... --path-id N`. Use `extract INPUT... -o DIR` for asset
files, or `batch JOB.json` for a budgeted extraction pipeline. All examples
below use only the synthetic fixtures built by the CLI integration tests.

## Protocol and discovery

The new `inspect`, `show` and `describe` commands write exactly one compact JSON
document plus a newline to stdout. Their successful documents have
`"format": "unityrs.<command>"` and `"version": 1`. Failures have
`"format": "unityrs.error"` and `"version": 1`. Fields are only added within a
version; readers ignore unknown fields. Ordering and content are deterministic:
no timestamps or canonicalized host paths. Paths retain the caller's spelling;
file names inside containers retain their stored spelling. Invalid UTF-8 paths,
names and strings use the canonical `{"t":"str","hex":"..."}` representation.

`describe` lists all nine subcommands, each option's value form, default (a
string, or null when absent), help, usage, stdout behavior and exit codes.
Legacy commands retain their original parsers and output contracts. `-h` and
`--help` on a new command return the JSON description. New options use separate
values (`--limit 10`), with `--` ending options; repeated `--class` and `--name`
are supported, other duplicate options are usage errors.

This projection of `unityrs describe` shows its discovery fields; the full
response, including every command and option, is checked in as
[describe.json](../crates/unityrs-cli/tests/fixtures/agent/describe.json).

<!-- example:describe -->
```json
{
  "batch_schema": "unityrs batch --schema",
  "format": "unityrs.describe",
  "schemas": [
    "inspect",
    "show",
    "describe",
    "error"
  ],
  "version": 1
}
```
<!-- /example:describe -->

`unityrs describe --schema inspect|show|describe|error` prints the corresponding
JSON Schema, draft 2020-12. Schema responses are standard schema documents;
the `format`/`version` envelope applies to protocol documents. Protocol envelope
and header properties permit additions; value tags require their documented
payload shapes, recursively. For the existing batch job schema, use
`unityrs batch --schema`; it is not duplicated here.

## Inspect and pagination

`inspect INPUT... [--class CLASS]... [--name GLOB]... [--limit N] [--cursor TOKEN]`
lists each input with a `files` array or an error, plus object headers.
`files` lists the root and its descendants once in container order, including
resource files. Each entry carries its kind, Unity version and platform
(null where inapplicable). Each SerializedFile carries its own version and
platform, so mixed-version containers need no guessed aggregate platform.

Headers carry the input's zero-based index, SerializedFile name, signed path ID,
class ID and name (`Unknown` for an unmapped ID), object name, container path,
and serialized byte size. Absent names and paths are null. Names come from a
bounded typetree read's `m_Name`; objects with unavailable or unreadable trees
have null names. Budget refusals fail that input. Inspection does not decode
images, meshes or audio. Name discovery can still require reading an object's
typetree; the input budgets bound this work.

Class names and integer IDs follow `extract --asset-filter`; repeated classes
form a union. Name globs use `dump --name`'s case-sensitive Python fnmatch syntax
(`*`, `?`, `[abc]`, `[!abc]`), applied to object names. Repeated globs form a
union; class and name filters intersect. Unnamed objects do not match a name
filter. Inputs retain argument order; files retain container order; objects
are sorted by signed path ID within each file.

`unityrs inspect synthetic.assets --name alpha` produces:

<!-- example:inspect -->
```json
{
  "format": "unityrs.inspect",
  "inputs": [
    {
      "files": [
        {
          "file": "synthetic.assets",
          "kind": "AssetsFile",
          "target_platform": 13,
          "unity_version": "2022.3.0f1"
        }
      ],
      "path": "synthetic.assets"
    }
  ],
  "next_cursor": null,
  "objects": [
    {
      "byte_size": 24,
      "class_id": 49,
      "class_name": "TextAsset",
      "container_path": null,
      "file": "synthetic.assets",
      "input": 0,
      "name": "alpha",
      "path_id": -2
    }
  ],
  "total_objects": 1,
  "version": 1
}
```
<!-- /example:inspect -->

`total_objects` counts all objects after filtering, including those outside this
page. The page limit defaults to 200 and accepts 1 through 10,000. Pass a non-null
`next_cursor` unchanged to the next call; null means the last page. Cursors bind
the exact input spellings, their order, contents, filters, explicitly supplied
budget/TPK options, working directory and provider bytes. Keep these unchanged;
page size may change.
The cursor resumes after an object key (input index, exact file name, signed path
ID), with a file-occurrence tie-breaker for repeated names in nested containers,
rather than a numeric offset. Role-tagged fingerprint fields prevent an input
path from colliding with an option. Source hashing observes the exact loaded
snapshot, including joined split companions and compressed container bytes.
Provider hashing and parsing also share one read. Hashing counts against the
read/work budgets. If those bounds prevent fingerprinting an input, a page
requiring a cursor is refused with `budget` (or `input` if the source
cannot be opened). Foreign, malformed, edited or stale tokens are rejected.
Cursors are not authentication tokens or a snapshot across calls.
Enumeration is repeated per page, retaining only the page's headers, so a smaller
page bounds output rather than total scan work.

An input that fails is reported in its own `error` entry, contributes no objects,
and does not change inspect's success exit status. Inspect writes no stderr line
for these reported input failures. A shared TPK load failure is command-wide.
An invalid cursor or a refused response allocation also uses a command-wide
error document.

## Show and bounded values

`show INPUT... --path-id N [--file NAME] [--max-bytes N] [--max-items N]`
`[--max-depth N] [--bytes summary|full] [--value natural|canonical]` returns a
header and one typetree value.
Path IDs are signed integers. If several files contain the ID, the error's
`detail` array lists each candidate's input index and exact file name. Supply
`--file` to select a file. If input files themselves share that name, narrow the
input list. No match is `not_found`. A malformed target input is `input`.

`unityrs show synthetic.assets --path-id -2` produces:

<!-- example:show -->
```json
{
  "format": "unityrs.show",
  "object": {
    "byte_size": 24,
    "class_id": 49,
    "class_name": "TextAsset",
    "container_path": null,
    "file": "synthetic.assets",
    "input": 0,
    "name": "alpha",
    "path_id": -2
  },
  "truncated": {
    "binary": 0,
    "bytes": 0,
    "cuts": 0,
    "depth": 0,
    "items": 0
  },
  "value": {
    "m_Name": "alpha",
    "m_Script": "hello"
  },
  "version": 1
}
```
<!-- /example:show -->

The default `--value natural` emits ordinary JSON objects, arrays, strings,
booleans and numbers. Unique UTF-8 field names become object keys in typetree
order. Unity maps with unique UTF-8 string keys become objects in stored entry
order. The typetree identifies maps; ordinary sequences and tuples become arrays,
including arrays of pairs. Finite floats become JSON numbers,
including negative zero. Integers from -2^53 through 2^53 are bare numbers.
Exceptional values preserve their data with these small tags:

- Integers outside that range use `{"t":"i64","v":"-9223372036854775808"}` or
  `{"t":"u64","v":"18446744073709551615"}` with exact decimal strings.
- Nonfinite floats use `{"t":"f32","v":"7fc00042"}` or `f64`, with the
  original 8 or 16 hexadecimal bit digits, preserving NaN payloads.
- Invalid UTF-8 strings use `{"t":"str","hex":"ff61"}`.
- Field dictionaries with duplicate names, invalid UTF-8 keys, or the reserved
  keys `t` or `$unityrs` use `{"t":"dict","v":[[key,value],...]}`.
- Unity maps with duplicate, non-string, invalid UTF-8 or reserved keys use
  `{"t":"map","v":[[key,value],...]}`. Keys and values are recursively rendered,
  preserving arbitrary key values and entry order without string conversion.
  Item-clipped dictionaries and maps also use their respective wrappers so the
  `cut` property is separate from data.
- Byte runs use the summary or full hex form described below.

Use `--value canonical` for [the canonical dump model](CANONICAL_DUMP.md): field
dictionaries use `t: dict`, tuples use `t: tuple`, integers remain bare, and all
floats retain bit tags. Unity maps retain the canonical array of tagged pairs. Primitive arrays use bounded arrays of canonical scalar values.
Both views share display limits and truncation markers. This option affects
only `show`; the parity dump and value-case protocols retain their contracts.

Default limits are 65,536 bytes for the **whole compact success document including
its newline**, 256 retained data entries per array/map, and 32 nesting levels
(root depth zero). Raise them with `--max-bytes`, `--max-items`, and `--max-depth`;
maximum supported display depth is 128. Zero items or depth is useful for a
summary. These display limits do not replace the reader budgets below.

Every cut is a reserved marker object. Reserved keys in data use the dictionary
or map wrapper, keeping data unambiguous in either view:

- `{"$unityrs":"cut","reason":"depth","omitted":1}` replaces a subtree.
- `{"$unityrs":"cut","reason":"bytes","omitted":1}` replaces a value that
  cannot fit its allocated document space. The renderer reserves framing room
  conservatively; increasing the byte cap can reveal more data.
- `{"$unityrs":"cut","reason":"items","omitted":N}` marks N omitted entries.
  It is appended to a sequence, or stored as a map wrapper's `cut` property.
  Markers do not count toward the retained-data item limit.
- Byte runs (`TypelessData` and UInt8/char vectors) normally become
  `{"$unityrs":"bytes","kind":"bytes","length":100,"sha256":"...","prefix_hex":"..."}`.
  `kind` is `u8s` for byte vectors; the prefix is the first min(64, length) bytes.
  This is also a cut, even if the prefix includes the entire small byte run.
  Hashing counts against decode work. `--bytes full` emits canonical
  `{"t":"bytes","hex":"..."}` / `u8s` when they fit, otherwise the same summary.

The top-level `truncated` object counts **visible marker objects** by `bytes`,
`items`, `depth`, and `binary`; `cuts` is their sum. Cuts inside a replaced
subtree are not counted again. Omitted counts refer to immediate data entries,
not guessed descendant counts. Increase the relevant limit and ask again to
retrieve more. When even the header and one marker cannot fit `--max-bytes`,
show returns a `budget` error; error documents are outside the success byte cap.

## Errors, exit codes and resource budgets

The closed version-1 error code set is:

| Code | Meaning |
| --- | --- |
| `usage` | Missing, unknown, duplicate or invalid command arguments. |
| `input` | Input, provider, container or typetree read failed. |
| `budget` | Input/read/response memory, source bytes, work or depth refused. |
| `not_found` | No object matches the requested path ID and file. |
| `ambiguous` | Several objects match; `detail` lists candidates. |
| `bad_cursor` | Invalid cursor, changed inputs or changed listing options. |

A missing object produces this complete error document:

<!-- example:error -->
```json
{
  "code": "not_found",
  "format": "unityrs.error",
  "message": "no object with that path ID and file",
  "version": 1
}
```
<!-- /example:error -->

New commands exit 0 on success, 1 on operation failure, 2 on usage error. A
failure writes that JSON to stdout and one escaped human-readable line to
stderr. Input failures identify the input index and escaped caller path in their
message, with `detail.input` carrying the zero-based index. Inspect's per-input
failures remain exit 0. Usage errors identify the option, bad value and accepted
forms or range. Existing commands keep their own exit meanings; `describe` lists
them, including batch's distinct codes from [BATCH_SPEC.md](BATCH_SPEC.md).

Inspect and show use extract's budget option names and defaults:

| Option | Default | Bounds |
| --- | ---: | --- |
| `--max-input-memory` | 8589934592 | Charged bytes per input, and a separate response budget. |
| `--max-input-bytes` | 4294967296 | Source bytes per input, before loading. |
| `--max-decode-work` | 34359738368 | Work per input, and response rendering/hashing work. |
| `--max-tree-depth` | 256 | Reader graph/tree depth. |
| `--tpk FILE` | absent | Optional bounded schema provider for stripped trees. |

Pass larger numeric limits explicitly when needed. The input loader and JSON
text writer charge allocations before growth and use fallible allocation.
Show checks borrowed header and framing lengths against the output cap before
copying the header or rendering a value. An inspect response also retains input
metadata; the page limit bounds object headers, while the response memory budget
bounds all metadata and JSON buffers.
Inputs are loaded separately, preserving repeated caller paths and deterministic
input attribution. No implicit dependencies are fetched; provide all needed files.

## Extraction, Python and scope

Use `extract` when you need exported files and its JSON failure report. Use the
[batch engine](BATCH_SPEC.md) for explicit dependency graphs, parallel workers,
resource budgets, manifests, progress, directory/archive/object-store sinks,
retries and lifecycle controls. Batch's `--check` validates a job before running.
Its stdout and exits follow its existing specification.

The [Python package](../crates/unityrs-py/README.md) provides the UnityPy-compatible
API for in-process Python callers, including object/typetree access. The new
CLI protocol is intended for bounded exploration and process-driven tools.

Out of scope: an MCP server or other persistent transport, mutation/write-back
commands, remote input discovery, a class-specific interpretation of every
value, and changing the parity commands' argument or output semantics. The new
operations return JSON documents from CLI module functions without printing;
a later in-process transport can call the same functions.

## Golden examples and regeneration

The fixtures are generated in `crates/unityrs-cli/tests/agent/support.rs` using
`unityrs-testkit` only. Complete response goldens live in
`crates/unityrs-cli/tests/fixtures/agent/`. The `goldens` test checks these examples
against the exact documents (the describe example is the explicit projection
shown above). Regenerate both the goldens and these fenced examples with:

```sh
UNITYRS_UPDATE_AGENT_GOLDENS=1 cargo test -p unityrs-cli --test agent goldens
```

Review the changed golden files and guide together, then run the test without
the environment variable to check them.
