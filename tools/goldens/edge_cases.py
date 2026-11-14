"""Hand rules for the two originally inline cases and mixed exit severity.

The old inline command logs are unavailable. These transformations reproduce
their checked-in pins from oracle-derived texture bytes plus §3/§7.3 rules;
they never consult engine output. The bundle fixture construction itself is
in crates/unityrs-batch/tests/run_support/mod.rs::textures.
"""

import copy

from model import load_case, write_case


def generate():
    original = load_case("textures-five")
    source = next(t for t in original["texture_inputs"] if t["format"] == 4)
    output = next(
        r for r in original["normalized"]["rows"]["outputs"] if r["key"] == "RGBA32.png"
    )

    case = load_case("rules-zero")
    case.update(name="rules-cut-strings", fixture="synthetic:cut")
    # E40's source-string byte cap is 1024, applied before JSON escaping.
    name = 'CAB-"\\' + "a" * 1100
    case["serialized_name"] = name
    case["texture_inputs"] = [source]
    case["outputs"] = {"RGBA32.png": original["outputs"]["RGBA32.png"]}
    row = copy.deepcopy(output)
    row.update(path_id=1, serialized_file=name[:1024], cut=True)
    case["normalized"]["rows"]["outputs"] = [row]
    case["normalized"]["head"]["counts"]["outputs_written"] = 1
    write_case(case)

    case = load_case("rules-zero")
    case.update(name="rules-nested-repeated", fixture="synthetic:nested-repeated")
    # §3 excludes all repeated-CAB rows and nested bundle payloads from output.
    # §7.3 still records one diagnostic for the nested directory row.
    case["normalized"]["rows"]["diagnostics"] = [
        {
            "kind": "nested_container",
            "input": "x.bundle",
            "bundle": "x.bundle",
            "name": "nested",
            "cut": False,
        }
    ]
    case["normalized"]["head"]["run"]["nested_containers"] = 1
    write_case(case)

    case = load_case("rules-budget")
    case.update(name="rules-mixed-severity", fixture="synthetic:mixed-severity")
    # One dispatched input refuses export work (5); an unrelated missing
    # input adds a source/data failure (4). §9.1/§9.2 retain the more severe 5.
    case["spec"]["inputs"].append({"id": "z", "path": "$SOURCE:z", "deps": []})
    case["missing_sources"] = ["z"]
    missing = load_case("rules-failed-input")["normalized"]["rows"]
    bundle = copy.deepcopy(missing["bundles"][0])
    summary = copy.deepcopy(missing["inputs"][0])
    bundle["id"] = summary["id"] = "z"
    rows = case["normalized"]["rows"]
    rows["bundles"].append(bundle)
    rows["inputs"].append(summary)
    case["normalized"]["head"]["counts"].update(
        bundles=2,
        bundles_failed=1,
        inputs=2,
        inputs_failed=2,
    )
    write_case(case)
