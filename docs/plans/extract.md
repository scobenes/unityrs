# P1b plan: 2D batch extraction (draft)

**Status: draft, not reviewed.** Written 2026-11-09, when `docs/DESIGN.md` section 5 gave the 2D work a phase of its own. Nothing here is built. This plan gets its own review rounds, run until a round finds no MEDIUM or HIGH problem, before any of it is.

**What it builds on.**
- The core as merged in P1 (`docs/plans/core.md`), through the properties its section 4.1 lists. A need those properties do not cover is a request to the core, made through its integration rule, not an assumption.
- The batch engine, whose design is `docs/plans/batch.md`: registration, admission, eviction, the hand-off to sinks and refusal. This plan states what it needs from the engine (section 1.3, 1.8, 1.13) and does not design it.

**What it delivers.** `unityrs batch JOB.json`: one job of N bundles plus their dependencies, decoded in parallel under explicit budgets, with textures, sprites and NGUI atlases written as PNG files and atlas JSON to a directory, zip archives or an S3-compatible store, and a run manifest that says what was written, what was skipped and why. A release job publishes the binary. The first production consumers run on this phase; the plan names none, and their own acceptance runs (L10) are theirs.

**How requirements are written.** Each is numbered (E1, E2, ...). Each names the level it is checked at (`docs/DESIGN.md` section 6) and, once the plan is reviewed, the test that checks it. Where UnityPy 1.25.2 has the behaviour, UnityPy (or the oracle stack, `docs/DESIGN.md` section 3) is the reference. Where it has none (the sinks, the manifest, the cutter's JSON), the reference is the rule written here.

---

## 1. Requirements

### 1.1 The job spec and the CLI

- **E1.** A job is a JSON document with a `spec_version`. The binary refuses a version it does not know, before any work, with a named error.
- **E2.** `unityrs batch JOB.json` runs one job. `unityrs --version` prints the crate version and the highest `spec_version` the binary reads.
- **E3.** Exit codes tell apart: complete; complete with object-level skips; incomplete; a budget refusal; a bad spec; a crash. Their values are fixed in this plan's review and never reused for another meaning.
- **E4.** Progress is NDJSON on stdout, one line at least every few seconds while work runs, so a caller can use it as a heartbeat. Its schema is versioned with the spec.
- **E5.** The budgets of section 1.13 are required fields; a spec without them is refused (E1's error), and autodetection is only a capped fallback the spec opts into.

Levels: L9 for the CLI's behaviour on synthetic specs.

### 1.2 Inputs and downloads

- **E6.** An input is a local path or a URL.
- **E7.** A URL takes headers, a per-request timeout and a retry policy: which statuses and errors retry, the backoff, and the number of attempts.
- **E8.** `download_concurrency` bounds parallel downloads.
- **E9.** The spec may give a URL rule that fetches a dependency by name. A name found only in an AssetBundle's own dependency list is fetched by that rule.
- **E10.** A download that fails after its retries fails the input (or, for a dependency, every input that needs it: E13). The run is then `incomplete`.

Levels: L9, against a local test server that serves, delays, fails and truncates on command.

### 1.3 Dependencies and isolation

- **E11.** The spec carries a dependency graph: for each input, the names of its dependencies.
- **E12.** Under `EnvOptions::batch()`, an input's `.resS` reads and its container read its own bundle's same-named CAB first (core plan section 9, Q12; `docs/plans/batch.md` 3.9). `EnvOptions::unitypy()` keeps UnityPy's one registry.
- **E13.** A dependency that fails fails exactly the inputs that need it (`docs/plans/batch.md` 3.10).
- **E14.** Each dependency is parsed once per run, within `dep_cache_bytes` (the engine's goal, `docs/plans/batch.md` section 1).

Levels: L1 (env) and L9, on testkit scenes.

### 1.4 Decoding

- **E15.** BC7, DXT1, DXT5, RGBA32 and RGB24 decode bit-exact, with UnityPy 1.25.2's output modes: `RGB` for RGB24, `RGBA` for the other four (`PARITY.md` section 17's rows for these five).
- **E16.** `meshdecode`'s vertex-data unpack (every VertexFormat table, index formats and topologies, `get_triangles`), in f64, as `PARITY.md` section 15.3's rows marked P1b state. CompressedMesh and PackedBitVector are P2a.
- **E17.** Sprites render as UnityPy's `SpriteHelper` renders them (`PARITY.md` section 15.2): atlas render data, tight meshes, rotation, flips and the alpha merge. The affine-triangle path is a Tolerance row: its metric and threshold are stated in `PARITY.md` when this phase lands it.
- **E18.** Every decoder runs under `catch_unwind`. A panic is an object-level skip, recorded in the manifest, never a crashed run.

Levels: L5 on checked-in fixtures for the five formats and every sprite path; L5 on the nightly private corpus.

### 1.5 Images

- **E19.** A PNG writer byte-equal to Pillow 9.5.0's at its defaults, for modes `L`, `LA`, `RGB` and `RGBA` (`docs/DESIGN.md` section 4's `unityrs-imageio` row, L5b).
- **E20.** The visible-pixel hash, exactly: SHA-256 over the ASCII bytes `"{width}x{height}:"`, then the image's RGBA bytes in row order, where each pixel of alpha 0 is written as (0, 0, 0, 0). An image in another mode is converted to RGBA first, as Pillow 9.5.0's `convert("RGBA")` converts it.

Levels: L5b on checked-in fixtures for every mode and on the nightly corpus; E20 by a reference implementation in the parity tools, run on the same images.

### 1.6 The NGUI atlas cutter

- **E21.** Sheet scope `own_files_by_name`: the material's main-texture name, then the atlas name, matched among the input's own Texture2D objects only. A PPtr may cross into a dependency to read a name, never to take its texture.
- **E22.** A caller-given name pattern skips reference atlases.
- **E23.** A crop past the sheet's edge is padded with zeros, as Pillow 9.5.0's `Image.crop` pads.
- **E24.** The atlas JSON is byte-equal to CPython 3.11's `json.dumps(value, indent=2)` (its defaults otherwise: ASCII escapes, the float `repr`, `NaN` written as `NaN`) of a documented dict: keys `name`, `texture_width`, `texture_height`, `pixel_size`, `settings`, `sprite_count`, `sprites`, in that order, with typetree values written as read (pairs as arrays, f32 widened exactly). The layout is documented beside `docs/CANONICAL_DUMP.md` before it is built.
- **E25.** A value the layout cannot write (bytes) skips that atlas's JSON alone, recorded in the manifest. The crops stay.
- **E26.** Collisions are handled per atlas, and the atlas's winner also wins its sheet texture (`docs/DESIGN.md` section 4's `unityrs-export` row).

Levels: L5 and L5b for the crops, L7 for the JSON, on testkit-built atlases (an NGUI-shaped MonoBehaviour typetree is synthetic) and the nightly corpus.

### 1.7 Output naming

- **E27.** Key templates per output kind come from the spec. Each template says whether a name is written verbatim or through `safe()`.
- **E28.** `safe()` works per Unicode code point: every character outside `[A-Za-z0-9._-]` becomes `_`; then leading and trailing `.` are stripped; then an empty result becomes `unnamed`.
- **E29.** Inside one input, the first object of a given type and name takes the key, even when its decode then fails.
- **E30.** Inside one atlas, of two sprites with one `safe()` name, the later one wins.

Levels: L9.

### 1.8 Winners across inputs

- **E31.** When several inputs produce one key, the spec's `prefer` map, `{key: {input, md5}}`, decides:
  1. the candidate from the preferred input, if that input is in the run;
  2. otherwise the candidate with the preferred MD5;
  3. otherwise the first candidate input in bytewise name order, reported in the manifest as a **flip**.
- **E32.** An atlas, its crops and its JSON win as one unit. The atlas's `prefer` entry is its sheet key's.
- **E33.** "Pixels differ" means a different visible hash (E20). A collision whose candidates share a visible hash is reported, but not as a pixel difference.
- **E34.** The winners hold candidates until every input that can produce a key has run: a sink that holds output across jobs. `docs/plans/batch.md` 3.5's model must allow it.

Levels: L9.

### 1.9 Sinks

- **E35.** Directory.
- **E36.** Zip: stored members; member paths and their order from a caller-given map; spooled to a caller-given directory with a size cap and a free-space reserve; no archive written that would have no member.
- **E37.** Object store (S3-compatible):
  - credentials from the environment only, a session token included; no instance-metadata fallback;
  - path-style addressing available (bucket names with dots break virtual-hosted TLS);
  - Content-Type set on every PUT, and no other header;
  - files single-part, so the ETag is the MD5;
  - archives multipart; a failed or interrupted multipart upload is aborted, on SIGTERM too;
  - keys written byte for byte as given (empty segments, `.` and `..` segments, `%`, `#`, `~`, spaces, non-ASCII), or refused with a named error. Never rewritten.

Levels: L9 against a local S3-compatible test server, with its request log as the evidence for headers, addressing and keys.

### 1.10 The run manifest

- **E38.** The manifest's first write is status `running`, with the `run_id` and the attempt number. No output is written before it.
- **E39.** Its last write replaces it with the final status: `complete`, `complete_with_skips` or `incomplete`.
- **E40.** Per output: key, source input, MD5, visible hash, mode, size, status. Per collision: the key, each candidate (input, MD5, visible hash), the winner and the rule that chose it, flips marked. Also object-level skips, input failures, each archive's status and member count, timings and peak memory.
- **E41.** In an object store, Content-Type `application/json`.
- **E42.** No credential appears in the manifest, the progress stream or any error message.

Levels: L9, with a test that runs every sink with known credentials and searches every byte the run wrote or printed for them.

### 1.11 Resume and idempotence

- **E43.** A re-run with resume LISTs the output root once and skips a PUT whose key already holds the same MD5.
- **E44.** Every input is still read and decoded, so winners, archives and the manifest cover the whole job. There is no partial ("narrowed") re-run.

Levels: L9: a run killed at each stage, then resumed, writes the same keys and bytes as an uninterrupted run.

### 1.12 Lifetime

- **E45.** A wall-clock `deadline` in the spec. Past it the run stops, aborts open uploads, writes `incomplete` and exits.
- **E46.** The process exits when its parent dies (Linux `PR_SET_PDEATHSIG`, set by the binary itself) and, when the spec asks, when stdin reaches EOF.
- **E47.** SIGTERM: abort open multipart uploads, write `incomplete`, exit.

Levels: L9.

### 1.13 Budgets

- **E48.** `threads`, `download_concurrency`, `max_inflight_uncompressed_bytes`, `max_decoded_bytes`, `dep_cache_bytes`, the spool directory and the spool size are required (E5).
- **E49.** How they bound memory is `docs/plans/batch.md`'s design, once its open problems 3.1-3.10 are solved or shown wrong. This plan adds the spool (E36, E34) to what those budgets must cover.

### 1.14 Release CI

- **E50.** A tag builds the CLI for x86_64 Linux, glibc-linked (manylinux_2_28 or similar), with the vendored C libraries linked statically. Not musl: the core's memory meter charges in glibc's allocator chunks (`heap_cost`, core plan section 3.15), so the meter's bound holds only under glibc's allocator.
- **E51.** The tag publishes it as a GitHub release asset, with its SHA-256.
- **E52.** A release that a consumer may reference is never deleted.

Today `ci.yml` runs on pushes to main and on pull requests only; the release job is new.

### 1.15 Verification

- **E53.** L5 and L5b run in CI on checked-in fixtures for the five formats, sprites and every PNG mode.
- **E54.** L9 (the extractor tree) for the 2D outputs, in CI on testkit-built inputs.
- **E55.** The nightly private corpus run covers L5, L5b and the atlas JSON, against the oracle stack's output for the same inputs.
- **E56.** L10, a consumer's shadow run, is the consumer's: it gates that consumer's cutover, not this phase.

---

## 2. Decided here

| decision | why |
|---|---|
| Lossy WebP moves to P3 | No first consumer needs it, and it brings `libwebp-sys` and its licence review into the phase for nothing the phase ships. |
| CompressedMesh and PackedBitVector move to P2a | Sprites need the vertex-data unpack, not the compressed mesh; `PARITY.md`'s rows for the two now say P2a. |
| The release is glibc, not musl | E50's reason: the memory meter's unit is glibc's chunk. |
| No narrowed re-run | E44: a re-run that skips inputs cannot choose winners, fill archives or write a whole manifest. |

## 3. Open, for the review rounds

1. **Verbatim keys and `object_store`.** `object_store`'s `Path` normalises and refuses some of the keys E37 must write byte for byte. Use a lower-level S3 client, or refuse those keys with a named error, and say which keys each choice cannot write.
2. **The gate.** A P1-style gate table for this phase: each requirement's test, the fixture sets, and the nightly corpus run's pass rule (the oracle stack has no batch mode, so the oracle side of E55 is a script that runs UnityPy's extractor and the documented cutter and naming rules).
3. **The cutter's reference.** UnityPy has no NGUI cutter. E21-E26 are this plan's rules, and their reference is a Python implementation of them in the parity tools, run on the oracle stack. Its review is part of this plan's.
4. **Spec schema.** The JSON schema itself, its versioning rule, and how a later version may add a field without breaking a consumer pinned to an earlier binary.
5. **Exit code values and the progress schema** (E3, E4).

## 4. Before it is built

- `docs/plans/batch.md`'s open problems solved or shown wrong, in its own review rounds.
- This plan's review rounds, run until a round finds no MEDIUM or HIGH problem, with section 3 settled.
- A gate table (section 3, item 2) agreed before the first package starts.
