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
file names inside containers retain their stored spelling, subject to inspect's
explicit previews below. Invalid UTF-8 paths and names use the canonical
`{"t":"str","hex":"..."}` representation.
Value strings follow the rules for the selected view below.

`describe` lists all ten subcommands, each option's value form, default (a
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

`inspect INPUT... [--class CLASS]... [--name GLOB]... [--limit N]`
`[--max-bytes N] [--cursor TOKEN]`
lists each input with a `files` array or an error, plus object headers.
`files` lists the root and its descendants once in container order, including
resource files. Each entry carries its `file_index`, kind, Unity version and
platform (null where inapplicable). Each SerializedFile carries its own version and
platform, so mixed-version containers need no guessed aggregate platform.

Headers carry the input's zero-based index, the file's zero-based `file_index`,
SerializedFile name, signed path ID, class ID and name (`Unknown` for an unmapped ID), object name, container path,
and serialized byte size. Absent names and paths are null. Names come from a
bounded typetree read's `m_Name`; objects with unavailable or unreadable trees
have null names. Budget refusals fail that input. Inspection does not decode
images, meshes or audio. Name discovery can still require reading an object's
typetree; the input budgets bound this work.

Class names and integer IDs follow `extract --asset-filter`; repeated classes
form a union, deduplicated by numeric ID with a budgeted binary lookup per object.
Name globs use `dump --name`'s case-sensitive Python fnmatch syntax
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
          "file_index": 0,
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
      "file_index": 0,
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
page. The page limit defaults to 200 and accepts 1 through 10,000. The whole
compact success document, including its newline, is also bounded by
`--max-bytes` (default 65,536). A byte cut may return fewer objects than `--limit`.
Every listed object retains its full numeric `input`, `file_index` and `path_id`;
use those to address it even when its file name is a preview.

Input paths (including provider paths in errors), metadata file names and Unity
versions, and object file names, names and container paths longer than 128 source bytes use a
`"$unityrs":"preview"` object with the original byte `length`, retained `prefix`,
and omitted byte count `omitted`.
The prefix contains at most 128 source bytes without splitting a UTF-8 code
point; an invalid UTF-8 prefix uses the usual `t: str` hex form. `length` and
`omitted` count source bytes, not characters or escaped JSON bytes. Filtering
still uses the complete original name. Previews are bounded independently of
the page size. Input metadata is retained on every page; if the cap cannot hold
that metadata and one object's addressing/header plus any required cursor, the
command refuses with a clear `budget` error. It never emits a partial object or
an empty page with a continuation token. Error documents are outside the cap.

Pass a non-null `next_cursor` unchanged to the next call; null means the last
page. Cursors bind
the exact input spellings, their order, contents, filters, explicitly supplied
budget/TPK options and provider bytes. Keep these unchanged;
page size, response byte cap and working directory may change. Relative paths
must still resolve to the same loaded bytes.
The cursor resumes exactly after the last object emitted, including on a byte-cut
page. Its key contains the input index, a digest of the exact file name and signed
path ID, with a file-occurrence tie-breaker for repeated names in nested containers,
rather than a numeric offset. Role-tagged fingerprint fields prevent an input
path from colliding with an option. Source hashing observes the exact loaded
snapshot, including joined split companions and compressed container bytes.
Provider hashing and parsing also share one read. Hashing counts against the
read/work budgets. If those bounds prevent fingerprinting an input, a page
requiring a cursor is refused with `budget` (or `input` if the source
cannot be opened). Tokens with a wrong fingerprint, invalid checksum, malformed
payload or absent object key are rejected.
A cursor is the caller's own token over the caller's own inputs, so an edited
cursor can only show the caller another page of their own data; the checksum
guards against corruption, not tampering. Cursors are integrity-checked, not
authenticated; edit one and you get what you asked for. They are not a snapshot
across calls.
Enumeration is repeated per page, retaining only the page's headers, so a smaller
page bounds output rather than total scan work.

An input that fails is reported in its own `error` entry, contributes no objects,
and does not change inspect's success exit status. Inspect writes no stderr line
for these reported input failures. A shared TPK load failure is command-wide.
An invalid cursor or a refused response allocation also uses a command-wide
error document.

## Show and bounded values

`show INPUT... --path-id N [--file NAME] [--file-index INPUT:FILE]`
`[--max-bytes N] [--max-items N]`
`[--max-depth N] [--bytes summary|full] [--value natural|canonical]` returns a
header and one typetree value.
Path IDs are signed integers. If several files contain the ID, the error's
`detail` array lists each candidate's `input`, `file_index` and exact `file` name.
Supply `--file NAME` when the name is unique, or `--file-index INPUT:FILE` to
select the exact occurrence even inside nested containers or repeated inputs.
Both indices are zero-based; FILE is the position in that input's `files` array,
including the root and resource files. With both selectors, both must match.
An explicit `--file-index` skips loading every other input, so an unrelated
broken, missing or resource input cannot fail the selection. Without that
selector, all supplied inputs participate in finding candidates and must load.
For example, a header with `input: 0`, `file_index: 2`, `file: "CAB-a"` and
`path_id: -2` can be read with
`unityrs show nested.bundle --path-id -2 --file-index 0:2`.
No match is `not_found`. A malformed target input is `input`.

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
    "file_index": 0,
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
floats retain bit tags. Unity maps retain the canonical array of tagged pairs.
Nonempty float vectors pack into `{"t":"f32s","v":"3fc0000040200000"}`
for `[1.5, 2.5]` (or `f64s` for doubles). Empty vectors are `[]`. Invalid UTF-8
value strings, bytes and byte vectors use inline hex through 64 bytes, then
`{"t":"str|bytes|u8s","len":N,"sha256":"..."}` with the actual kind in `t`.
These canonical summaries are part of the dump model and do not count as cuts;
`--bytes` controls only the natural view. An untruncated canonical value is
byte-identical to the dump encoder.
For example, `unityrs show synthetic.assets --path-id -2 --value canonical`
returns `{"t":"dict","v":[["m_Name","alpha"],["m_Script","hello"]]}` in `value`.
Both views share display limits and truncation markers. Item-clipped float
vectors use arrays of canonical scalar tags followed by the item marker.
This option affects only `show`; the parity dump and value-case protocols retain their contracts.

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
- In the natural view, byte runs (`TypelessData` and UInt8/char vectors) use
  `{"$unityrs":"bytes","kind":"bytes","length":100,"sha256":"...","prefix_hex":"..."}`.
  `kind` is `u8s` for byte vectors; the prefix is the first min(64, length) bytes.
  This is also a cut, even if the prefix includes the entire small byte run.
  `--bytes full` emits
  `{"t":"bytes","hex":"..."}` / `u8s` when they fit, otherwise the same summary.

String validation, scanning and escaping, hex encoding, number formatting,
marker construction and hashing count against decode work before they run.

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
  "message": "show: input 0 'synthetic.assets' --path-id '999': no object with that path ID and file",
  "version": 1
}
```
<!-- /example:error -->

New commands exit 0 on success, 1 on operation failure, 2 on usage error. A
failure writes that JSON to stdout and one escaped human-readable line to
stderr. Input failures identify the input index and escaped caller path in their
message, with `detail.input` carrying the zero-based index. Once an object is
known, read and rendering failures also carry `detail.file_index` and
`detail.path_id`. A shared provider failure identifies `--tpk` and its current
path in the message and `detail.tpk`; it is not attributed to an input, including
when `--file-index` skips other inputs. Provider paths use the same bounded
preview rule as inventory paths.
Inspect's per-input failures remain exit 0 when its response can be written.
Budget errors name the option to raise and its current numeric value, including
refusals during rendering after parsing succeeds. Response-wide errors identify
the first addressed object when available, otherwise the command and inputs. Fixed reader and
provider bounds explicitly say that no CLI option raises them; allocator
refusals advise reducing the input or response size or freeing host memory.
Selection errors repeat the requested `--path-id`, `--file` and `--file-index`
values when supplied. Cursor errors name `--cursor` and its bounded token
spelling, retain any provider or input context, and advise restarting without
the cursor. A resource refusal remains `budget` even during cursor replay.
Usage errors identify the option, bad value and accepted forms or range;
duplicate options report both values. Existing commands keep their own exit
meanings; `describe` lists them, including batch's distinct codes from [BATCH_SPEC.md](BATCH_SPEC.md).

Inspect and show use extract's budget option names and defaults:

| Option | Default | Bounds |
| --- | ---: | --- |
| `--max-input-memory` | 8589934592 | Charged bytes per input, and a separate response budget. |
| `--max-input-bytes` | 4294967296 | Source bytes per input, before loading. |
| `--max-decode-work` | 34359738368 | Work per input, and response rendering/hashing work. |
| `--max-tree-depth` | 256 | Compiled typetree depth. |
| `--tpk FILE` | absent | Optional bounded schema provider for stripped trees. |

Pass larger numeric limits explicitly when needed. `--max-tree-depth` controls
compiled typetrees; runtime value depth and container depth retain the reader's
fixed defaults and have no agent CLI override. The input loader and JSON
text writer charge allocations before growth and use fallible allocation.
Show checks borrowed header and framing lengths against the output cap before
copying the header or rendering a value. An inspect response also retains input
metadata with bounded string previews; `--limit` and `--max-bytes` bound the
returned page, while the response memory budget bounds all staging buffers.
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

Out of scope: other persistent transports, mutation/write-back
commands, remote input discovery, a class-specific interpretation of every
value, and changing the parity commands' argument or output semantics. The new
operations return JSON documents from CLI module functions without printing;
the MCP transport calls the same functions in process.

## MCP server

Register a local server with Claude Code:

```sh
claude mcp add unityrs -- unityrs mcp --root /path/to/assets
```

Clients such as Claude Desktop that read a JSON server configuration can use:

```json
{
  "mcpServers": {
    "unityrs": {
      "command": "unityrs",
      "args": ["mcp", "--root", "/path/to/assets", "--out", "/path/to/exports"]
    }
  }
}
```

The server implements the [MCP 2025-11-25 lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)
over stdio: one UTF-8 JSON-RPC 2.0 object per line, with stdout reserved for
responses and stderr for diagnostics. It supports `initialize`,
`notifications/initialized`, `ping`, `tools/list`, and `tools/call`. The supported
version is `2025-11-25`; that version is echoed when requested, and offered for
any unsupported version. Send `notifications/initialized` before using tools.
Batch arrays are rejected (`-32600`), following this revision's removal of
batching. Notifications produce no response; malformed `notifications/initialized` does not
enable tools. Repeated `initialize` and tool requests before initialization are
refused. Parse errors use `-32700`, invalid
requests `-32600`, unknown methods `-32601`, malformed method parameters `-32602`.
Requests retain their string or integer IDs. EOF exits successfully; malformed
and oversized messages are refused and the next line is served. Calls execute
serially. No network, shell, subprocess, or environment-variable expansion is
used by tool calls.

`tools/list` supplies draft 2020-12 `inputSchema` and `outputSchema` for each tool.
Input names replace CLI hyphens with underscores. Each result has a single
`content` text item containing the JSON document, and the same object in
`structuredContent`. Operation and tool argument failures set `isError: true` with a
`unityrs.error` document. Argument errors identify the argument, supplied value
and accepted type, form or range; ceiling errors identify the server maximum.
Output schemas include that error variant. Malformed `tools/call` envelopes
(including a non-object `arguments`) and unknown tool names remain JSON-RPC
invalid parameters.

| Tool | Arguments | Document |
| --- | --- | --- |
| `unityrs_describe` | Optional `schema` (`inspect`, `show`, `describe`, `error`). | `unityrs.describe`, or the requested JSON Schema. |
| `unityrs_inspect` | Required `inputs` string array; optional `class` and `name` arrays, `limit`, `cursor`, `max_bytes`, budgets and `tpk`. | Version-1 `unityrs.inspect`, with per-input errors and `next_cursor`. |
| `unityrs_show` | Required `inputs` and signed `path_id`; optional `file`, `file_index` (`INPUT:FILE`), `value`, `bytes`, `max_bytes`, `max_items`, `max_depth`, budgets and `tpk`. | Version-1 `unityrs.show`. |
| `unityrs_extract` | Required `inputs` and relative `output` subdirectory; extract's options, budgets and provider paths. Repeatable `asset_filter` is an array; switches are booleans. | Extract report: `written` and `failures`; names use Unicode strings or raw-string tags below. Listed only with server `--out`. |
| `unityrs_batch_check` | Required `job` object. | Existing batch check `exit` result, including `code: 0` and `detail: "memory_bound ..."`; invalid jobs return `unityrs.error`. No batch execution. |

Input and output option schemas are generated from the CLI parser tables. For
example, call `unityrs_inspect` with `{"inputs":["/path/to/assets/synthetic.assets"],"limit":1}`,
then keep its inputs, filters and budget options unchanged when supplying its
`next_cursor` as `cursor`.

Extract report `path` and `serialized_file` names are Unicode strings or,
for invalid UTF-8 bytes, the lossless `{"t":"str","hex":"..."}` form used by
the agent interface. Both text and structured content, and a saved `report`,
use this form. The parity `extract` command keeps its existing output contract.
`ignore_first_container_dirs` accepts exactly signed 64-bit integers,
-9,223,372,036,854,775,808 through 9,223,372,036,854,775,807; negative values slice
from the end of the container path's slash components.

At least one startup `--root DIR` is required; repeat it to permit several roots.
Both startup `--root` and `--out` reject symlinks in every component, including
the named directory itself, and reject `..`. Spell platform directory aliases
using their real paths. Missing output directories are created with `mkdirat`
and opened component by component with `openat(O_DIRECTORY|O_NOFOLLOW)` against
held parents; a symlink ancestor is refused before creating its descendants.
Relative input paths resolve against the server's working directory. Absolute
paths must be beneath a configured root. Each component is opened relative to
its held parent descriptor with `O_NOFOLLOW`; `..`, symlinks (including symlinks
within roots), special files and paths outside all roots are refused. Roots
are held for the session. Provider files (`tpk` and `vorbis_headers`) and every
split companion follow the same rules. Input leaves must be regular files;
nonblocking opens refuse FIFOs, sockets and device nodes without waiting for
a writer. Loaded bytes retain their input budget. TPK decompression, provider
tables and lazy tree compilation share the request memory and decode-work
limits; the provider's fixed decompression bound also remains in force.
No implicit dependencies are fetched. Batch check validates path and URL strings
as job data without opening, downloading, resolving credentials, or executing them.

Extraction uses the existing held-descriptor writer. A request's `output` must
be relative beneath `--out`; parent components and symlink parents are refused.
`report`, when supplied, is relative to that extraction subdirectory and must
end in a filename. Output, report and asset-derived paths also reject NUL,
backslashes and colon-bearing components. Writes replace existing leaves. Confinement attaches to the inodes reached on open:
a concurrent directory rename moves those inodes and their files together,
including outside the original root pathname. A directory moved after opening
stays reachable through its held descriptor. Files already hard-linked inside
a root are readable; confinement does not establish their provenance.

Startup budget maxima use the ordinary `--max-input-memory`, `--max-input-bytes`,
`--max-decode-work`, and `--max-tree-depth` options and defaults above. Startup
also accepts display ceilings `--max-bytes 65536`, `--max-items 256`,
`--max-depth 32`, and `--limit 200`. Explicit request values above any ceiling
are refused with `isError: true`, code `budget`; omitted values use the smaller
of the command default and server ceiling. A request may lower these limits.
Job checks additionally cap the serialized job at 64 MiB and at the server's
input byte ceiling, reserving 64 bytes per job byte, 65,536 fixed bytes, 16,384 bytes per bundle,
and the exact active/stage slot table sizes before preparation. Memory reservations
must fit the server ceiling; work is split conservatively between validation
and preparation, with the existing batch view-work cap also retained.
Tool document framing, escaping and the serialized response buffer share the
command's finite response meter; documents are serialized directly into both
MCP result fields without an intermediate JSON tree.

`--max-request-bytes` defaults to 1,048,576 bytes per physical line, including
newline; oversized lines are drained with bounded storage. A partial final line
at EOF is accepted within the same cap. The decoder constructs the request
under three additional caps: `max(32, floor(line cap / 64))` nodes (each value
and object key counts once), `floor(line cap / 2)` total decoded UTF-8 bytes
across strings and keys, and depth 32 with the root at zero. At the default
line cap these are 16,384 nodes and 524,288 string bytes. They bound tree
amplification to a small constant of the line cap, before dispatch and before
any unrestricted JSON tree can be built. Cap refusals are invalid requests
(`-32600`). Duplicate keys anywhere, including batch jobs, are refused before
values can collapse: envelope duplicates use `-32600`, nested parameter
and argument duplicates use `-32602`. Decoder refusals use a null ID because
an unambiguous request has not been decoded. These caps are startup-derived;
requests cannot raise them. `--max-response-bytes`
defaults to 4,194,304 bytes per complete protocol response including newline.
Both are at most 67,108,864; request cap is at least 256, and response cap must
be at least request cap + 1024 so even a large request ID fits a bounded error.
A response that cannot fit is replaced by a budget tool failure (or JSON-RPC
internal error for non-tool responses), preserving its ID. Framing caps are
startup-only. `unityrs mcp --help` lists the startup options on stderr.

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
