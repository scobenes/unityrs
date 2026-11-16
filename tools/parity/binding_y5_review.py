"""Synthetic Y5 review observations, including explicit safety differences."""
from types import SimpleNamespace as NS
from unittest import mock

from binding_y5_tools import extractor, _reader_file, _capture
import scratch


def observe_destination_order(module):
    E = extractor(module)
    file = _reader_file(module)
    result = {}
    with scratch.folder("y5-destination-order-") as folder:
        for label, objects, accepted in (
            ("unsupported", [file.objects[3]], None),
            ("filtered_reader", [file.objects[1]], lambda _: False),
            ("filtered_instance", [file.objects[1]], lambda obj: hasattr(obj, "parse_as_object")),
        ):
            dst = folder / label
            with mock.patch.object(module, "load", return_value=NS(objects=objects)):
                result[label] = _capture(lambda: E.extract_assets(
                    b"", dst, use_container=False, asset_filter=accepted), folder)
            result[label + "_exists"] = dst.exists()
    return result


def observe_failed_image(module, corrected=False):
    from PIL import Image
    E = extractor(module)
    with scratch.folder("y5-failed-image-") as folder:
        target = folder / "image.png"
        Image.new("RGBA", (1, 1), (1, 2, 3, 4)).save(target)
        previous = target.read_bytes()
        obj = NS(m_Width=1, image=Image.new("RGBA", (1, 0)),
                 assets_file="file", object_reader=NS(path_id=1))
        outcome = _capture(lambda: E.exportTexture2D(obj, str(folder / "image")), folder)
        data = target.read_bytes()
        if corrected and module.__name__ == "UnityPy":
            data = previous
        return {"outcome": outcome, "bytes": data.hex(), "size": len(data),
                "preserved": data == previous,
                "leaves": sorted(path.name for path in folder.iterdir())}
