"""The URP material and Android texture fixture runs: their inventory and built-bundle contract.

`anim_replay.py complete` hands a run.json whose run names a variant other than
`builtin` to `complete_variant`. These runs record no replays. Each case is a
bundle checked through the same `unityrs inspect` and `show` reads as the
Built-in static cases (tools/fixtures/unity/Editor/UnityrsUrpFixtures.cs and
UnityrsAndroidFixtures.cs build them).
"""

from math import ceil

from anim_fixture_contract import ASSETS, STATIC_CASES, entries, prefab_components
from anim_replay_complete import BundleView, bundle_digest, check_handoff_stamps, check_static

URP_CASE = "urp-materials"
ANDROID_CASE = "android-textures"
ANDROID_TARGET = ("Android", 13)
URP_ASSETS = {"materials.prefab": 1, "lit.mat": 21, "unlit.mat": 21, "base.asset": 28, "normal.asset": 28}
URP_TEXTURES = {"base.asset": (4, 4), "normal.asset": (2, 2)}
# node: material asset, shader, required keywords, texture slots (asset, scale, offset), floats, colours
URP_MATERIALS = {
    "Lit": ("lit.mat", "Universal Render Pipeline/Lit", {"_NORMALMAP", "_EMISSION"},
            {"_BaseMap": ("base.asset", (2, 3), (.25, .5)), "_BumpMap": ("normal.asset", (1, 1), (0, 0))},
            {"_Metallic": .625, "_Smoothness": .375, "_BumpScale": 1.5},
            {"_BaseColor": (.25, .5, .75, 1), "_EmissionColor": (.125, .25, .5, 1)}),
    "Unlit": ("unlit.mat", "Universal Render Pipeline/Unlit", {"_ALPHATEST_ON"},
              {"_BaseMap": ("base.asset", (1, 1), (0, 0))},
              {"_AlphaClip": 1, "_Cutoff": .375}, {"_BaseColor": (1, .5, .25, 1)}),
}
# Android override -> built TextureFormat, block edge and bytes per block.
ASTC_4X4, ASTC_6X6, ASTC_8X8 = (48, 4, 16), (50, 6, 16), (51, 8, 16)
ETC_RGB4, ETC2_RGB, ETC2_RGBA1, ETC2_RGBA8 = (34, 4, 8), (45, 4, 8), (46, 4, 8), (47, 4, 16)
ANDROID_TEXTURES = {
    "astc-4x4.png": (64, 64, ASTC_4X4), "astc-6x6.png": (40, 24, ASTC_6X6), "astc-8x8.png": (64, 64, ASTC_8X8),
    "etc-rgb4.png": (64, 64, ETC_RGB4), "etc2-rgb4.png": (64, 64, ETC2_RGB),
    "etc2-rgb4-punchthrough.png": (64, 64, ETC2_RGBA1), "etc2-rgba8.png": (64, 64, ETC2_RGBA8),
}
VARIANTS = {
    "urp": {URP_CASE: URP_CASE},
    "android": {case: bundle for case, (bundle, _) in STATIC_CASES.items()} | {ANDROID_CASE: ANDROID_CASE},
}


def vector(axes, values):
    return dict(zip(axes, values, strict=True))


def check_inventory(folder, manifest, run, replay):
    variant = run["variant"]
    replay.require(variant in VARIANTS, f"unknown fixture run variant in run.json: {variant!r}")
    if variant == "android":
        replay.require((run["build_target"], run["build_target_id"]) == ANDROID_TARGET,
                       "the Android fixture run must target Android (13)")
    cases, errors, seen = VARIANTS[variant], [], set()
    for row in manifest["cases"]:
        replay.require(isinstance(row, dict) and set(row) == {"name", "status", "reason"}
                       and all(isinstance(value, str) for value in row.values()), "invalid run.json case row")
        name, status, reason = row["name"], row["status"], row["reason"]
        replay.require(name in cases and name not in seen, f"unknown or duplicate {variant} run.json case: {name}")
        replay.require(status in {"recorded", "failed", "skipped", "not_covered"}
                       and (status == "recorded" or reason.strip()), f"invalid run.json case status or reason: {name}")
        seen.add(name)
        if status != "recorded":
            errors.append(f"{name}: {status} ({reason})")
    errors.extend(f"missing run.json case: {name}" for name in sorted(cases.keys() - seen))
    pending = sorted(path.relative_to(folder) for path in (folder / "replays").rglob("*.pending.json"))
    errors.extend(f"unexpected pending recording in a {variant} run: {path}" for path in pending)
    replay.require(not errors, f"incomplete {variant} fixture run:\n" + "\n".join(errors))
    bundles = folder / "bundles"
    replay.require(bundles.is_dir() and {path.name for path in bundles.iterdir()} == set(cases.values()),
                   f"unexpected {variant} fixture bundle inventory")
    check_handoff_stamps(folder, run, replay)
    return cases


def shader_name(view, key):
    """m_ParsedForm.m_Name, read at depth 2: a built shader's own m_Name is empty and its form is large."""
    file, path = key
    doc = view.document("show", ["--file-index", f"0:{file}", "--path-id", str(path), "--max-depth", "2",
                                 "--max-items", "64", "--max-bytes", "1048576", "--value", "canonical"],
                        f"{view.path.name}.{file}.{path}.shader.json")
    view.require(doc["object"] == view.headers[key], f"show identity disagrees with inspect: {key}")

    def field(value, name):
        view.require(isinstance(value, dict) and value.get("t") == "dict", f"unreadable built shader: {key}")
        found = [item for field_name, item in value["v"] if field_name == name]
        view.require(len(found) == 1, f"built shader has no {name}: {key}")
        return found[0]

    name = field(field(doc["value"], "m_ParsedForm"), "m_Name")
    view.require(isinstance(name, str), f"unreadable built shader name: {key}")
    return name


def keywords(view, value):
    if "m_ValidKeywords" in value:
        valid = value["m_ValidKeywords"]
        view.require(isinstance(valid, list) and all(isinstance(word, str) for word in valid),
                     "invalid built material keywords")
        return set(valid)
    words = value.get("m_ShaderKeywords")
    view.require(isinstance(words, str), "missing built material keywords")
    return set(words.split())


def check_urp(view):
    built = {asset: view.asset(ASSETS + "urp/" + asset, class_id, ignore_case=True)
             for asset, class_id in URP_ASSETS.items()}
    for asset, size in URP_TEXTURES.items():
        image = view.value(built[asset])
        view.require((image["m_Width"], image["m_Height"]) == size, f"built URP texture size mismatch: {asset}")
    nodes = prefab_components(view, built["materials.prefab"])
    for node, (asset, shader, words, textures, floats, colors) in URP_MATERIALS.items():
        material = built[asset]
        view.require(node in nodes and 23 in nodes[node], f"missing built URP renderer {node}")
        renderer = nodes[node][23]
        view.require([view.pointer(renderer, ptr, {21}) for ptr in view.value(renderer)["m_Materials"]] == [material],
                     f"built URP renderer material mismatch: {node}")
        value = view.value(material)
        shader_key = view.pointer(material, value["m_Shader"], {48}, within_bundle=True)
        view.require(shader_key is not None, f"missing built URP shader: {asset}")
        name = shader_name(view, shader_key)
        view.require(name == shader, f"built URP shader mismatch in {asset}: {name!r}, expected {shader!r}")
        missing = words - keywords(view, value)
        view.require(not missing, f"built URP keywords missing from {asset}: {', '.join(sorted(missing))}")
        props = value["m_SavedProperties"]
        slots = entries(props["m_TexEnvs"])
        for slot, (texture, scale, offset) in textures.items():
            env = slots.get(slot)
            view.require(env is not None and view.pointer(material, env["m_Texture"], {28}) == built[texture]
                         and env["m_Scale"] == vector("xy", scale)
                         and env["m_Offset"] == vector("xy", offset),
                         f"built URP texture slot mismatch: {asset} {slot}")
        stored = entries(props["m_Floats"])
        for prop, expected in floats.items():
            view.require(stored.get(prop) == expected, f"built URP float mismatch: {asset} {prop}")
        stored = entries(props["m_Colors"])
        for prop, expected in colors.items():
            view.require(stored.get(prop) == vector("rgba", expected),
                         f"built URP colour mismatch: {asset} {prop}")


def byte_length(view, data):
    """Length of a canonical byte value: inline hex or the summarised length."""
    if isinstance(data, list):
        return len(data)
    view.require(isinstance(data, dict) and data.get("t") in ("bytes", "u8s"), "invalid built texture image data")
    return data["len"] if "len" in data else len(data["hex"]) // 2


def check_android_textures(view):
    files = {row["file"].rsplit("/", 1)[-1].lower() for row in view.files.values()}
    for asset, (width, height, (texture_format, block, block_bytes)) in ANDROID_TEXTURES.items():
        image = view.value(view.asset(ASSETS + "android/" + asset, 28, ignore_case=True))
        size = ceil(width / block) * ceil(height / block) * block_bytes
        view.require((image["m_Width"], image["m_Height"]) == (width, height),
                     f"built Android texture size mismatch: {asset}")
        view.require(image["m_TextureFormat"] == texture_format,
                     f"built Android texture format mismatch: {asset} is {image['m_TextureFormat']}, "
                     f"expected {texture_format} from its Android override")
        view.require(image.get("m_MipCount") == 1 and image.get("m_CompleteImageSize") == size,
                     f"built Android texture data size mismatch: {asset}")
        inline = byte_length(view, image.get("image data", {"t": "bytes", "hex": ""}))
        stream = image.get("m_StreamData") or {}
        streamed = stream.get("size", 0)
        if inline:
            view.require(inline == size and streamed == 0, f"built Android texture data size mismatch: {asset}")
        else:
            path = stream.get("path", "")
            view.require(streamed == size and isinstance(path, str) and path.rsplit("/", 1)[-1].lower() in files,
                         f"built Android texture stream mismatch: {asset}")


def complete_variant(folder, manifest, run, unityrs, inspect_dir, replay):
    """Validate a URP or Android run's bundles; return the summary line."""
    cases = check_inventory(folder, manifest, run, replay)
    bundle_dir, views = folder / "bundles", {}

    def view(bundle):
        path = bundle_dir / bundle
        digest = bundle_digest(path)
        return views.setdefault((bundle, digest), BundleView(path, digest, 0, unityrs, inspect_dir, replay, run))

    try:
        if run["variant"] == "urp":
            check_urp(view(URP_CASE))
        else:
            check_static(bundle_dir, views, unityrs, inspect_dir, replay, run)
            check_android_textures(view(ANDROID_CASE))
    except (KeyError, TypeError, IndexError, AttributeError, OSError) as exc:
        raise ValueError(f"{run['variant']} fixture run: {exc}") from exc
    for bundle_view in views.values():
        replay.require(bundle_digest(bundle_view.path) == bundle_view.digest,
                       f"bundle changed during inspection: {bundle_view.path.name}")
    return f"Validated {len(cases)} {run['variant']} fixture case(s)."
