# Package C requests

Changes package C needs in files it does not own, and places where the plan
and UnityPy 1.25.2 disagree (UnityPy wins; the plan text should follow it).
Each item says what to change, why, the exact signature where there is one,
and what C does until the change lands.

## 1. `DecodeBuf`: expose the block's meter and segment cap (scaffold, `budget.rs`)

- **What.** Two accessors on `DecodeBuf`:
  ```rust
  impl DecodeBuf {
      /// The meter this buffer charges.
      #[must_use]
      pub fn meter(&self) -> &Arc<Meter>;
      /// The cap of the current segment (`begin_block`'s `cap`).
      #[must_use]
      pub fn segment_cap(&self) -> u64;
  }
  ```
- **Why.** Plan 3.6 charges the dictionary an LZMA block names to the input's
  meter, capped with the block. A `BlockCodec` receives only
  `out: &mut DecodeBuf`, so the LZMA codec cannot reach either.
- **Until then.** `codec::Lzma` allocates the dictionary adaptively, so it
  stays within a constant plus twice the block's charged output. The first
  attempt uses the declared size clamped to 1-8 MiB and capped by the named
  size. An attempt that fails after outgrowing its dictionary decodes again
  with at least twice the output so far. The output is identical to a decode
  with the named dictionary (`codecs.rs`:
  `lzma_matches_beyond_the_first_dictionary_decode_again_with_a_larger_one`).
  The dictionary itself stays uncharged; it is liblzma's C heap.

## 2. The version pattern is `(.+?)?`, not `(.)?` (plan, `docs/CANONICAL_DUMP.md`, `version.rs` module doc)

- **What.** In `docs/plans/core.md` 3.4 (line 223), `docs/CANONICAL_DUMP.md`
  4.2 (line 160) and the scaffold's module doc of `crates/unityrs-core/src/version.rs`
  (line 4), write the pattern as UnityPy has it:
  `^(\d+)\.(\d+)\.(\d+)(.+?)?(\d+)?(.*)$` under DOTALL
  (UnityPy helpers/UnityVersion.py:7-10).
- **Why.** UnityPy wins on a disagreement. The two patterns accept the same
  strings and give the same groups, because the greedy `build` group leaves a
  non-digit after it and the lazy type group takes exactly that one character.
  The docs should still quote the pattern UnityPy actually uses.
  `version::parse_packed` already cites the UnityPy form.

## 3. The oracle dump crashes on a format-5 external (package E, `tools/parity/unitypy_dump.py:328`)

- **What.** Encode a missing `temp_empty` instead of passing `None` to
  `C.enc_name`. For example:
  `C.enc_name(e.temp_empty) if e.temp_empty is not None else None`, with
  `docs/CANONICAL_DUMP.md` saying what a format-5 external records there.
- **Why.** UnityPy reads `temp_empty` only from format 6
  (files/SerializedFile.py:66, 79-80) and leaves it `None` below that, so a
  format-5 file with an external crashes the dump.
- **Until then.** The testkit matrix writes externals only from format 6
  (`crates/unityrs-testkit/src/bundle.rs`, `matrix_file`). Format 5 is still
  covered at L0-L1 without externals.

## 4. `DecodeBuf::new` does not count its initial room as decode work (scaffold, `budget.rs`)

- **What.** In a `LimitKind::BlockOutput` segment, count `initial` as decode
  work the way `reserve` does, through `grow_counted`, so the order is charge,
  then count, then allocate. Otherwise, document that it does not.
- **Why.** Plan 3.6 counts every byte a block decode is granted room for.
  `grow_counted` returns early when capacity already suffices, so room made
  ahead by `new(_, initial, ..)` is never counted when the decode later fills
  it.
- **Until then.** `BundleFile::new_buf` (C) creates the buffer empty with a
  `BlockOutput` segment and calls `reserve(initial)`, which counts the room
  before allocating it. `bundles.rs`: `a_refused_block_is_not_kept` checks
  that refused growths count no work, and
  `decode_work_stops_a_lazy_read_and_a_reopen_on_the_same_meter_shares_it`
  checks the bound.

## 5. Plan 7.4: the format-8 `metadata_size > file_size` site is unreachable (plan)

- **What.** In the 7.4 test list, record that UnityPy's detection keeps this
  file raw: it requires `file_size >= metadata_size` and both at most the
  length (helpers/ImportHelper.py:110-120). Neither a loose file nor a bundle
  entry reaches `reader.Position = file_size - metadata_size`
  (files/SerializedFile.py:266) with a negative value.
- **Why.** The oracle records `CAB-f8-metadata-past-start` as a
  `ResourceFile`, and C matches it.
- **Also.** A format-22 `data_offset` above the file is kept raw by the same
  check. The `byte_start` past `i64::MAX` is therefore built through an object
  row (`CAB-f22-byte-start-past-i64`), which detection does not bound. Its L1
  has no rows, so `serialized.rs` (`a_byte_start_past_i64_is_kept_whole`)
  pins the value.

## 6. Plan 7.4: a 32,767-byte version string is not read whole (plan)

- **What.** In the 7.4 test list, say "the longest string
  `read_string_to_null` reads whole: 32,766 bytes" in place of "32,767
  bytes".
- **Why.** `read_string_to_null(32767)` searches `[pos, pos + 32767)`
  (streams/EndianBinaryReader.py:275-284). A NUL after 32,767 bytes is
  outside that window, so the read returns `read(32767)` and leaves the NUL
  for the next field.
- **Done in C.** `files_memory.rs` uses 32,766-byte strings, both a file's own
  string and the bundle's engine string taken by an empty own string. It
  checks that every row's version is a slice of one string.

## 7. Non-ASCII digit versions wait for A's `tables::ND_ZEROS` (integration)

- **What.** After A's generated table is merged, confirm that
  `files_fixtures.rs::non_ascii_digit_versions_match_the_oracle` runs in full.
  It does so once `ND_ZEROS.len() >= 2`; until then it only parses each case
  to check for panics. The fixture holds 11 such cases, including the
  4,300-digit and 4,301-digit groups in each field.
- **Why.** The scaffold seeds ASCII `0` only, so these cases cannot match
  before A lands. C's files carry no `STUB(` marker for this.

## 8. A caller's `Bytes` allocates its shared header inside the core (plan 3.15, informational)

- **What.** Add this to the per-input constant in plan 3.15, next to
  `Meter::OWN_HEAP`: a `Bytes` made from a `Vec` (the input, or a `RawStr`
  name such as `RawStr::from(&str)`) allocates its 24-byte shared header
  (32 bytes in glibc chunks) on its first clone. When the core makes that
  first clone, the allocation is uncharged. It happens once per caller
  buffer, so at most two per top-level input. Alternatively, have the
  public constructors build the shared form up front.
- **Why.** C's counting-allocator test (`files_memory.rs`) clones the caller's
  name and data before its baseline, so the header counts as the caller's
  heap.

## 9. The eager AssetBundle check needs D's `container::read_container` (integration)

- **What.** `container::read_container` is still the scaffold's `todo!()`.
  `SerializedFile::parse` calls it under `eager_asset_bundle` (UnityPy's
  default) for a file holding a class-142 object, so any such file panics
  until D lands. C's matrix has no class-142 object.
- **Ask.** When D's container lands, add a class-142 object to the testkit
  matrix and record it with `tools/fixtures/files.py`. The C throughput probe
  ran with `eager_asset_bundle` off for this reason.

## 10. Testkit API beyond the plan's list (informational, for D and E)

C's builders also export the following, which D and E may use:

- `bundle::build_with_signature(spec: &BundleSpec, signature: &[u8]) -> Vec<u8>`: `UnityWeb` and `UnityRaw` headers.
- `bundle::matrix() -> Vec<(String, Vec<u8>)>`: the 96 L0-L1 inputs.
- `bundle::compress(codec: TestCodec, data: &[u8]) -> Vec<u8>`.
- `bundle::LZMA_DICT: u32`.
- `TestCodec::id(self) -> u8`.
- `serialized::one_object(class_id: i32, nodes: Vec<NodeSpec>, data: Vec<u8>) -> SerializedSpec`.

## 11. Encrypted bundles (P3, informational)

UnityPy builds an `ArchiveStorageDecryptor` from the header's encryption flags
(files/BundleFile.py:124). C refuses those bundles as `Unsupported`; the
matrix inputs are `new-flags-encrypted.bundle` and
`old-flags-encrypted.bundle`. A caller's `FileContext::decryptor` still
decrypts blocks flagged `0x100` (files/BundleFile.py:512-513), in place in a
charged copy (`bundles.rs`: `a_flagged_block_is_decrypted_in_a_charged_copy`).
