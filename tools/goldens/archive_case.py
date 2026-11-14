"""Archive goldens from `docs/BATCH_SPEC.md` 1.7, 4.7 and 8, by hand rules.

Member payloads are the pinned loose PNG bytes of `textures-five`. Archive
bytes come from a `struct` encoder written from section 8 alone, then are read
back member by member with CPython's `zipfile`, an independent reader. Python's
ZIP writer is never the byte oracle: it chooses other metadata.
"""

import copy
import hashlib
import io
import struct
import zipfile
import zlib

from model import load_case, write_case

ZIP32 = 0xFFFFFFFF
ZIP16 = 0xFFFF
LONG = "a" * 253  # "{name}" plus this is one segment over NAME_MAX (255)


def encode(members):
    """Section 8 for small archives: no zip64 field can overflow here."""
    local = bytearray()
    central = bytearray()
    for name, payload in members:
        name = name.encode("utf-8")
        assert len(payload) < ZIP32 and len(local) < ZIP32
        crc = zlib.crc32(payload)
        header = (0x0800, 0, 0x0000, 0x0021, crc, len(payload), len(payload))
        central += struct.pack(
            "<IHH4H3I5H2I",
            0x02014B50, 20, 20, *header, len(name), 0, 0, 0, 0, 0, len(local),
        )
        central += name
        local += struct.pack("<IH4H3I2H", 0x04034B50, 20, *header, len(name), 0)
        local += name + payload
    count = len(members)
    assert count < ZIP16 and len(central) < ZIP32
    end = struct.pack(
        "<I4H2IH", 0x06054B50, 0, 0, count, count, len(central), len(local), 0
    )
    return bytes(local + central + end)


def read_back(data, members):
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        assert archive.testzip() is None
        infos = archive.infolist()
        assert [i.filename for i in infos] == [name for name, _ in members]
        for info, (_, payload) in zip(infos, members):
            assert info.compress_type == zipfile.ZIP_STORED
            assert info.flag_bits == 0x0800
            assert info.date_time == (1980, 1, 1, 0, 0, 0)
            assert info.extra == b"" and info.comment == b""
            assert info.internal_attr == info.external_attr == 0
            assert archive.read(info) == payload


def slash_first(path):
    """Section 4.6's order: `/` below every other byte."""
    return [0 if b == 0x2F else b + 1 for b in path.encode("utf-8")]


def archive(pins, key, template, outputs):
    """Rows and bytes of one archive over one `texture` member template."""
    paths = []
    for out in outputs:
        name = out["key"][: -len(".png")]
        path = template.replace("{name}", name)
        too_long = any(len(s.encode()) > 255 for s in path.split("/"))
        paths.append((path, out, "unwritable_path(segment_too_long)" if too_long else None))
    # Display order: path in `/`-first order, then loose key and input.
    paths.sort(key=lambda p: (slash_first(p[0]), p[1]["key"].encode(), p[1]["input"]))
    member_rows = []
    included = []
    seen = set()
    for path, out, reason in paths:
        if reason is None and path in seen:
            reason = "duplicate_member"
        if reason is None:
            seen.add(path)
            included.append((path, out))
        member_rows.append(
            {
                "archive": key,
                "path": path,
                "key": out["key"],
                "input": out["input"],
                "md5": out["md5"],
                "included": reason is None,
                "reason": reason,
                "cut": False,
            }
        )
    payloads = [(path, bytes.fromhex(pins[out["key"]]["hex"])) for path, out in included]
    data = encode(payloads)
    read_back(data, payloads)
    row = {
        "key": key,
        "status": "empty" if not included else "published",
        "reason": None,
        "members": len(included),
        "left_out": len(member_rows) - len(included),
        "md5": None,
        "etag": None,
        "size": None,
    }
    pin = None
    if included:
        row["md5"] = hashlib.md5(data).hexdigest()
        row["size"] = len(data)
        pin = {"md5": row["md5"], "hex": data.hex(), "members": [p for p, _ in payloads]}
    return row, member_rows, pin


def build(base, name, archives, pins):
    c = copy.deepcopy(base)
    c["name"] = name
    c["spec"]["archives"] = [
        {"key": key, "members": [{"kind": "texture", "path": template}]}
        for key, template in archives
    ]
    rows = c["normalized"]["rows"]
    # Winners are members whether or not publication is withheld (4.7).
    outputs = [o for o in rows["outputs"] if o["status"] in ("published", "withheld")]
    results = sorted(
        (archive(pins, key, template, outputs) for key, template in archives),
        key=lambda r: r[0]["key"].encode(),
    )
    rows["archives"] = [r[0] for r in results]
    rows["archive_members"] = [m for r in results for m in r[1]]
    c["archives"] = {r[0]["key"]: r[2] for r in results if r[2]}
    h = c["normalized"]["head"]
    h["counts"]["archives_written"] = len(c["archives"])
    if any(not m["included"] for m in rows["archive_members"]) and h["exit_code"] == 0:
        # Section 9.2: a member phase 6 refused is a 3.
        h.update(status="complete_with_skips", exit_code=3)
    c["exit_code"] = h["exit_code"]
    return c


def generate():
    five = load_case("textures-five")
    # Spec order differs from key order; one key is nested, one path non-ASCII.
    write_case(
        build(
            five,
            "archives-textures",
            [("packs/all.zip", "tex/{name}.png"), ("a.zip", "é/{name}")],
            five["outputs"],
        )
    )
    # A duplicate path keeps the first key; only left-out paths stay `empty`.
    write_case(
        build(
            five,
            "archives-left-out",
            [("dup.zip", "same.png"), ("none.zip", "{name}" + LONG)],
            five["outputs"],
        )
    )
    # Withholding comes after `empty` in precedence (section 4.7).
    withheld = build(
        load_case("rules-withheld"),
        "archives-withheld",
        [("w.zip", "{name}.png"), ("e.zip", "{name}" + LONG)],
        five["outputs"],
    )
    for row in withheld["normalized"]["rows"]["archives"]:
        if row["key"] == "w.zip":
            row.update(
                status="withheld", reason="publish_incomplete", md5=None, size=None
            )
    withheld["archives"] = {}
    withheld["normalized"]["head"]["counts"]["archives_written"] = 0
    write_case(withheld)
