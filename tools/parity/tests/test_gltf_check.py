"""Checks independent of the Rust writer, with deliberately broken payloads."""
import contextlib
import io
import json
import math
import struct
import sys
import unittest
from unittest.mock import patch
from tools.parity import gltf_check as g
from tools.parity import scratch


def fixture(normals=True, reversed_winding=False):
    positions = [[-1.,0.,0.],[0.,1.,0.],[0.,0.,0.]]
    data = struct.pack('<9f', *(v for row in positions for v in row))
    doc = {'bufferViews':[{'byteLength':36}], 'accessors':[{'bufferView':0,'componentType':5126,'type':'VEC3','count':3}]}
    p = {'attributes':{'POSITION':0}}
    if normals:
        data += struct.pack('<9f', *([0.,0.,1.]*3))
        doc['bufferViews'].append({'byteOffset':36,'byteLength':36})
        doc['accessors'].append({'bufferView':1,'componentType':5126,'type':'VEC3','count':3})
        p['attributes']['NORMAL'] = 1
    ids = [0,1,2] if reversed_winding else [2,1,0]
    doc['bufferViews'].append({'byteOffset':len(data),'byteLength':6})
    data += struct.pack('<3H', *ids)
    p['indices'] = len(doc['accessors'])
    doc['accessors'].append({'bufferView':len(doc['bufferViews'])-1,'componentType':5123,'type':'SCALAR','count':3})
    doc['meshes'] = [{'primitives':[p]}]
    return doc, data


def repeated_triangles(agree, disagree):
    doc, data = fixture()
    ids = [2, 1, 0] * agree + [0, 1, 2] * disagree
    doc['accessors'][2]['count'] = len(ids)
    doc['bufferViews'][2]['byteLength'] = len(ids) * 2
    return doc, data[:72] + struct.pack('<' + 'H' * len(ids), *ids)


def attribute_fixture(attributes):
    """Pack literal output rows; expectations never come from the checker."""
    doc = {'bufferViews': [], 'accessors': []}
    binary = b''
    ids = {}
    for semantic, rows in attributes.items():
        width = len(rows[0])
        code = 'H' if semantic == 'JOINTS_0' else 'f'
        binary += bytes(-len(binary) % 4)
        data = struct.pack('<' + code * (len(rows) * width),
                           *(v for row in rows for v in row))
        ids[semantic] = len(doc['accessors'])
        doc['bufferViews'].append({'byteOffset': len(binary), 'byteLength': len(data)})
        doc['accessors'].append({'bufferView': len(doc['bufferViews']) - 1,
                                 'componentType': 5123 if code == 'H' else 5126,
                                 'type': 'VEC' + str(width), 'count': len(rows)})
        binary += data
    doc['meshes'] = [{'primitives': [{'attributes': ids}]}]
    return doc, binary


def animation_fixture(path, rows, mode="LINEAR"):
    shape = {"translation": "VEC3", "rotation": "VEC4", "weights": "SCALAR"}[path]
    output = [v for row in rows for v in row]
    data = struct.pack(f"<{2 + len(output)}f", 0.25, 1.25, *output)
    doc = {
        "bufferViews": [{"byteLength": 8}, {"byteOffset": 8, "byteLength": len(data) - 8}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "type": "SCALAR", "count": 2,
             "min": [0.25], "max": [1.25]},
            {"bufferView": 1, "componentType": 5126, "type": shape,
             "count": len(output) if path == "weights" else len(rows)},
        ],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"targets": [{"POSITION": 4}, {"POSITION": 5}]}]}],
        "animations": [{
            "samplers": [{"input": 0, "output": 1, "interpolation": mode}],
            "channels": [{"sampler": 0, "target": {"node": 0, "path": path}}],
        }],
    }
    return doc, data


class Checks(unittest.TestCase):
    def test_animation_reconstructs_step_and_linear_morph_rows(self):
        rows = [[0.25, 0.5], [0.75, -0.25]]
        for mode in ("STEP", "LINEAR"):
            doc, data = animation_fixture("weights", rows, mode)
            expected = {"animation": 0, "node": 0, "path": "weights", "interpolation": mode,
                        "times": [0.25, 1.25], "values": rows,
                        "samples": [[0.75, rows[0] if mode == "STEP" else [0.5, 0.125]]]}
            self.assertEqual(g.check_accessors(doc, data, [expected]), 1)
            wrong = dict(expected, samples=[[0.75, [0., 0.]]])
            with self.assertRaisesRegex(AssertionError, "animation .* sample"):
                g.check_accessors(doc, data, [wrong])
            channel = g.decode_animation_channel(doc, data, 0, 0, "weights")
            self.assertEqual(g.sample_animation(channel, 0.), rows[0])
            self.assertEqual(g.sample_animation(channel, 0.75),
                             rows[0] if mode == "STEP" else [0.5, 0.125])
            if mode == "STEP":
                self.assertEqual(g.sample_animation(channel, math.nextafter(1.25, 0.)), rows[0])
            self.assertEqual(g.sample_animation(channel, 1.25), rows[1])
            self.assertEqual(g.sample_animation(channel, 2.), rows[1])

    def test_animation_reconstructs_quaternion_slerp_off_key(self):
        s = math.sqrt(0.5)
        for sign in (1, -1):
            doc, data = animation_fixture("rotation", [[0., 0., 0., 1.], [0., 0., sign*s, sign*s]])
            channel = g.decode_animation_channel(doc, data, 0, 0, "rotation")
            result = g.sample_animation(channel, 0.5, rotation=True)
            for actual, expected in zip(result, [0., 0., math.sin(math.pi/16), math.cos(math.pi/16)]):
                self.assertAlmostEqual(actual, expected, places=14)

    def test_animation_reconstruction_checks_time_bounds_order_and_output_count(self):
        doc, data = animation_fixture("translation", [[1., 2., 3.], [4., 5., 6.]])
        with self.assertRaisesRegex(ValueError, "time order"):
            g.decode_animation_channel(doc, data[:4] * 2 + data[8:], 0, 0, "translation")
        doc["accessors"][0]["max"] = [1.5]
        with self.assertRaisesRegex(ValueError, "time bounds"):
            g.decode_animation_channel(doc, data, 0, 0, "translation")
        doc["accessors"][0]["max"] = [1.25]
        doc["accessors"][1]["count"] = 1
        with self.assertRaisesRegex(ValueError, "output dimensions"):
            g.decode_animation_channel(doc, data, 0, 0, "translation")

    def test_animation_time_bounds_use_accessor_f32_precision(self):
        doc, data = animation_fixture("translation", [[1., 2., 3.], [4., 5., 6.]])
        doc["accessors"][0].update(min=[0.1], max=[0.2])
        # IEEE 754 binary32 encodings of decimal 0.1 and 0.2, little-endian.
        data = bytes.fromhex("cdcccc3d cdcc4c3e") + data[8:]
        _, times, _ = g.decode_animation_channel(doc, data, 0, 0, "translation")
        self.assertEqual(times, [0.10000000149011612, 0.20000000298023224])

    def test_animation_rejects_non_unit_and_zero_quaternions(self):
        for invalid in ([0., 0., 0., 2.], [0., 0., 0., 0.], [0., 0., 0.5, 0.5],
                        [0., 0., 0., 1.0000001192092896]):
            with self.subTest(invalid=invalid):
                doc, data = animation_fixture("rotation", [[0., 0., 0., 1.], invalid])
                with self.assertRaisesRegex(ValueError, "unit quaternion"):
                    g.decode_animation_channel(doc, data, 0, 0, "rotation")
        with self.assertRaises(ValueError):
            g.unit_quaternion([0., 0., 0., 0.])

    def test_animation_requires_dense_views_that_fit_the_counts(self):
        doc, data = animation_fixture("translation", [[1., 2., 3.], [4., 5., 6.]])
        timing = doc["accessors"][0]
        for count in (3, 1_000_000_000):
            with self.subTest(count=count):
                timing["count"] = count
                with self.assertRaisesRegex(ValueError, "accessor view range"):
                    g.decode_animation_channel(doc, data, 0, 0, "translation")
        timing["count"] = 2
        del timing["bufferView"]
        with self.assertRaisesRegex(ValueError, "dense buffer view"):
            g.decode_animation_channel(doc, data, 0, 0, "translation")

    def test_animation_views_cannot_have_byte_stride_or_target(self):
        for index in (0, 1):
            for field, value in (("byteStride", 16), ("target", 34962)):
                with self.subTest(index=index, field=field):
                    doc, data = animation_fixture("translation", [[1., 2., 3.], [4., 5., 6.]])
                    doc["bufferViews"][index][field] = value
                    with self.assertRaisesRegex(ValueError, field):
                        g.decode_animation_channel(doc, data, 0, 0, "translation")

    def test_animation_checks_skin_and_mesh_attachments(self):
        doc, data = animation_fixture("translation", [[1., 2., 3.], [4., 5., 6.]])
        doc["nodes"][0]["skin"] = 0
        with self.assertRaisesRegex(ValueError, "TRS targets a skinned node"):
            g.decode_animation_channel(doc, data, 0, 0, "translation")
        doc, data = animation_fixture("weights", [[0.25, 0.5], [0.75, -0.25]])
        del doc["nodes"][0]["mesh"]
        with self.assertRaisesRegex(ValueError, "weights need a mesh"):
            g.decode_animation_channel(doc, data, 0, 0, "weights")

    def check_attributes(self, source, output, normalise=True):
        return g.check_accessors(*attribute_fixture(output),
                                 [{'mesh': 0, 'primitive': 0, 'attributes': source,
                                   'complete_texcoords': True}],
                                 normalise=normalise)

    def test_partial_uv_expectations_leave_unspecified_sets_unexamined(self):
        output = {'TEXCOORD_0': [[.5, .75]], 'TEXCOORD_1': [[.25, .5]]}
        for semantic, source in (('TEXCOORD_0', [[.5, .25]]),
                                  ('TEXCOORD_1', [[.25, .5]])):
            self.assertEqual(g.check_accessors(
                *attribute_fixture(output),
                [{'mesh': 0, 'primitive': 0, 'attributes': {semantic: source}}]), 1)
        # Repair needs the whole source UV family, even with the default
        # comparison settings. A bad set left in the output must fail.
        expected = [{'mesh': 0, 'primitive': 0, 'attributes': {
            'TEXCOORD_0': [[float('nan'), .25]]}}]
        with self.assertRaises(AssertionError):
            g.check_accessors(*attribute_fixture({'TEXCOORD_0': [[float('nan'), .75]]}),
                              expected)

    def test_colors_clamp_before_f32_rounding_including_custom_colors(self):
        for semantic in ('COLOR_0', '_COLOR_0'):
            source = {semantic: [[-2., .5, 1e300, 2.]]}
            self.assertEqual(self.check_attributes(source, {semantic: [[0., .5, 1., 1.]]}), 1)
            with self.assertRaises(AssertionError):
                self.check_attributes(source, {semantic: [[-2., .5, float('inf'), 2.]]})
            self.assertEqual(self.check_attributes(
                source, {semantic: [[-2., .5, float('inf'), 2.]]}, normalise=False), 1)

    def test_nonfinite_colors_are_omitted_only_in_normalised_mode(self):
        for semantic in ('COLOR_0', '_COLOR_0'):
            for value in (float('nan'), float('inf'), -float('inf')):
                source = {semantic: [[value, 0., 0., 1.]]}
                self.assertEqual(self.check_attributes(source, {}), 1)
                with self.assertRaises(AssertionError):
                    self.check_attributes(source, source)
                self.assertEqual(self.check_attributes(source, source, normalise=False), 1)

    def test_invalid_uv_sets_are_dropped_and_survivors_compacted(self):
        for invalid in (float('nan'), float('inf'), 1e300):
            source = {'TEXCOORD_0': [[invalid, .25]],
                      'TEXCOORD_2': [[.25, .5]], 'TEXCOORD_7': [[.5, .75]]}
            output = {'TEXCOORD_0': [[.25, .5]], 'TEXCOORD_1': [[.5, .25]]}
            self.assertEqual(self.check_attributes(source, output), 1)
            with self.assertRaises(AssertionError):
                self.check_attributes(source, {'TEXCOORD_2': [[.25, .5]],
                                               'TEXCOORD_7': [[.5, .25]]})
            with self.assertRaises(AssertionError):
                self.check_attributes(source, dict(output, TEXCOORD_2=[[0., 0.]]))
            self.assertEqual(self.check_attributes({'TEXCOORD_0': [[invalid, .25]]}, {}), 1)

    def test_uv_gaps_are_zero_filled_without_an_invalid_set_even_in_raw_mode(self):
        source = {'TEXCOORD_2': [[.5, .25]]}
        output = {'TEXCOORD_0': [[0., 0.]], 'TEXCOORD_1': [[0., 0.]],
                  'TEXCOORD_2': [[.5, .75]]}
        for normalise in (True, False):
            self.assertEqual(self.check_attributes(source, output, normalise), 1)
            with self.assertRaises(AssertionError):
                self.check_attributes(source, {'TEXCOORD_0': [[.5, .75]]}, normalise)
        raw = {'TEXCOORD_0': [[float('inf'), .75]], 'TEXCOORD_1': [[0., 0.]],
               'TEXCOORD_2': [[.5, .75]]}
        self.assertEqual(self.check_attributes(dict(source, TEXCOORD_0=[[1e300, .25]]),
                                              raw, normalise=False), 1)

    def test_short_skin_rows_are_padded_before_weight_and_joint_repairs(self):
        source = {'JOINTS_0': [[3, 4], [5, 6]], 'WEIGHTS_0': [[2., 2.], [-1., 0.]]}
        output = {'JOINTS_0': [[3, 4, 0, 0], [0, 0, 0, 0]],
                  'WEIGHTS_0': [[.5, .5, 0., 0.], [1., 0., 0., 0.]]}
        self.assertEqual(self.check_attributes(source, output), 1)
        raw = {'JOINTS_0': [[3, 4, 0, 0], [5, 6, 0, 0]],
               'WEIGHTS_0': [[2., 2., 0., 0.], [-1., 0., 0., 0.]]}
        self.assertEqual(self.check_attributes(source, raw, normalise=False), 1)
        for normalise, actual in ((True, output), (False, raw)):
            broken = dict(actual, JOINTS_0=[[3, 4, 1, 0], actual['JOINTS_0'][1]])
            with self.assertRaises(AssertionError):
                self.check_attributes(source, broken, normalise)
        for normalise in (True, False):
            self.assertEqual(self.check_attributes(
                {'JOINTS_0': [[3]]}, {'JOINTS_0': [[3, 0, 0, 0]],
                                      'WEIGHTS_0': [[1., 0., 0., 0.]]}, normalise), 1)
            with self.assertRaises(AssertionError):
                self.check_attributes({'JOINTS_0': [[3]]},
                                      {'JOINTS_0': [[3, 0, 0, 0]],
                                       'WEIGHTS_0': [[0., 0., 0., 0.]]}, normalise)

    def test_orientation_pass(self):
        self.assertEqual(g.orientation(*fixture())[0]['status'],'pass')

    def test_winding_failure(self):
        with self.assertRaises(AssertionError):
            g.orientation(*fixture(reversed_winding=True))

    def test_no_normals_is_na(self):
        self.assertEqual(g.orientation(*fixture(normals=False))[0]['status'],'N/A')

    def test_degenerate_is_na(self):
        doc, data = fixture()
        data = bytes(36) + data[36:]
        self.assertEqual(g.orientation(doc,data)[0]['status'],'N/A')

    def test_empty_primitive_with_normals_is_na(self):
        doc, data = fixture()
        for item in doc['accessors']:
            item['count'] = 0
        self.assertEqual(g.orientation(doc, data), [dict(
            mesh=0, primitive=0, eligible=0, agree=0,
            agreement_fraction=None, status='N/A')])

    def test_reporting_keeps_multiple_failures_and_na(self):
        doc, data = fixture(reversed_winding=True)
        bad = doc['meshes'][0]['primitives'][0]
        doc['meshes'] = [{'primitives': [bad, bad]},
                         {'primitives': [{'attributes': {'POSITION': 0}}]}]
        self.assertEqual(g.orientation(doc, data, strict=False), [
            dict(mesh=0, primitive=0, eligible=1, agree=0,
                 agreement_fraction=0., status='fail'),
            dict(mesh=0, primitive=1, eligible=1, agree=0,
                 agreement_fraction=0., status='fail'),
            dict(mesh=1, primitive=0, eligible=0, agree=0,
                 agreement_fraction=None, status='N/A')])

    def test_orientation_threshold_includes_exactly_ninety_nine_percent(self):
        result = g.orientation(*repeated_triangles(99, 1))[0]
        self.assertEqual((result['status'], result['agreement_fraction']), ('pass', .99))
        with self.assertRaises(AssertionError):
            g.orientation(*repeated_triangles(98, 2))
        result = g.orientation(*repeated_triangles(98, 2), strict=False)[0]
        self.assertEqual((result['status'], result['agreement_fraction']), ('fail', .98))

    def test_area_threshold_and_nonpositive_dot_do_not_count_as_agreement(self):
        doc, data = fixture()
        # Bounds diagonal squared rounds to 1; f32 rounding puts this area
        # just below the 1e-12 cutoff, and the next case just above it.
        tiny = struct.pack('<9f', 0., 0., 0., 1., 0., 0., 0., 2e-12, 0.)
        self.assertEqual(g.orientation(doc, tiny + data[36:])[0]['status'], 'N/A')
        tiny = struct.pack('<9f', 0., 0., 0., 1., 0., 0., 0., 2.001e-12, 0.)
        self.assertEqual(g.orientation(doc, tiny + data[36:], strict=False)[0]['eligible'], 1)
        orthogonal = data[:36] + struct.pack('<9f', *([1., 0., 0.] * 3)) + data[72:]
        result = g.orientation(doc, orthogonal, strict=False)[0]
        self.assertEqual((result['eligible'], result['agreement_fraction'], result['status']),
                         (1, 0., 'fail'))

    def test_reporting_cli_counts_failures_without_disabling_default_gate(self):
        doc, data = fixture(reversed_winding=True)
        doc['meshes'][0]['primitives'] *= 2
        doc['meshes'][0]['primitives'].append({'attributes': {'POSITION': 0}})
        encoded = json.dumps(doc).encode()
        encoded += b' ' * (-len(encoded) % 4)
        data += bytes(-len(data) % 4)
        payload = (struct.pack('<I4s', len(encoded), b'JSON') + encoded
                   + struct.pack('<I4s', len(data), b'BIN\0') + data)
        path = scratch.for_case(self).joinpath('orientation.glb')
        path.write_bytes(b'glTF' + struct.pack('<II', 2, 12 + len(payload)) + payload)
        out = io.StringIO()
        with patch.object(sys, 'argv', ['gltf_check.py', str(path), '--report']), contextlib.redirect_stdout(out):
            g.main()
        report = json.loads(out.getvalue())
        self.assertEqual(report['orientation_summary'], {'pass': 0, 'fail': 2, 'not_applicable': 1})
        self.assertEqual(len(report['orientation']), 3)
        with patch.object(sys, 'argv', ['gltf_check.py', str(path)]), self.assertRaises(AssertionError):
            g.main()

    def test_accessor_values(self):
        doc, data = fixture()
        expected = [{'mesh':0,'primitive':0,'attributes':{'POSITION':[[1.,0.,0.],[0.,1.,0.],[0.,0.,0.]]},'indices':[[0,1,2]]}]
        self.assertEqual(g.check_accessors(doc,data,expected),1)
        expected[0]['attributes']['POSITION'][0][0] = 2
        with self.assertRaises(AssertionError):
            g.check_accessors(doc,data,expected)

    def test_invalid_normals_require_normal_and_tangent_omissions(self):
        for normal in ([0., 0., 0.], [float("inf"), 0., 0.], [1e-50, 0., 0.]):
            doc, data = fixture(normals=False)
            expected = [{"mesh": 0, "primitive": 0, "attributes": {
                "NORMAL": [normal] * 3, "TANGENT": [[1., 0., 0., 1.]] * 3}}]
            self.assertEqual(g.check_accessors(doc, data, expected), 1)
            # Base attribute omissions also remove those morph semantics.
            doc["meshes"][0]["primitives"][0]["targets"] = [{"POSITION": 0}]
            expected[0]["targets"] = [{"POSITION": [[1.,0.,0.],[0.,1.,0.],[0.,0.,0.]],
                                       "NORMAL": [[2.,0.,0.]]*3, "TANGENT": [[3.,0.,0.]]*3}]
            self.assertEqual(g.check_accessors(doc, data, expected), 1)
            doc["meshes"][0]["primitives"][0]["targets"][0]["NORMAL"] = 0
            with self.assertRaisesRegex(AssertionError, "morph NORMAL.*absent"):
                g.check_accessors(doc, data, expected)
            del doc["meshes"][0]["primitives"][0]["targets"][0]["NORMAL"]
            doc["meshes"][0]["primitives"][0]["attributes"]["TANGENT"] = 0
            with self.assertRaisesRegex(AssertionError, "TANGENT.*absent"):
                g.check_accessors(doc, data, expected)
            del doc["meshes"][0]["primitives"][0]["attributes"]["TANGENT"]
            doc["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = 0
            with self.assertRaisesRegex(AssertionError, "NORMAL.*absent"):
                g.check_accessors(doc, data, expected)

    def test_invalid_tangents_require_only_tangent_omission(self):
        doc, data = fixture()
        expected = [{"mesh": 0, "primitive": 0, "attributes": {
            "NORMAL": [[0., 0., 1.]] * 3, "TANGENT": [[0., 0., 0., 1.]] * 3}}]
        self.assertEqual(g.check_accessors(doc, data, expected), 1)
        doc["meshes"][0]["primitives"][0]["attributes"]["TANGENT"] = 0
        with self.assertRaisesRegex(AssertionError, "TANGENT.*absent"):
            g.check_accessors(doc, data, expected)

    def test_sparse(self):
        doc = {'bufferViews':[{'byteLength':1},{'byteOffset':4,'byteLength':12}], 'accessors':[{'componentType':5126,'type':'VEC3','count':3,'sparse':{'count':1,'indices':{'bufferView':0,'componentType':5121},'values':{'bufferView':1}}}]}
        self.assertEqual(g.accessor(doc,b'\x02\0\0\0'+struct.pack('<3f',1,2,3),0),[[0]*3,[0]*3,[1,2,3]])

    def test_stride_offset(self):
        doc = {'bufferViews':[{'byteLength':32,'byteStride':16}], 'accessors':[{'bufferView':0,'byteOffset':4,'componentType':5126,'type':'VEC3','count':2}]}
        self.assertEqual(g.accessor(doc,struct.pack('<8f',99,1,2,3,99,4,5,6),0),[[1,2,3],[4,5,6]])

    def test_accessor_range(self):
        doc, data = fixture()
        doc['bufferViews'][0]['byteLength'] = 4
        with self.assertRaises(ValueError):
            g.accessor(doc,data,0)

    def test_uv_and_tangent_conversion(self):
        self.assertEqual(g.converted('TEXCOORD_0',[[.5,.25]]),[[.5,.75]])
        self.assertEqual(g.converted('TANGENT',[[2.,0.,0.,1.]]),[[-1.,0.,0.,-1.]])

    def test_normal_and_weight_normalisation(self):
        self.assertEqual(g.converted('NORMAL',[[0.,0.,2.]]),[[0.,0.,1.]])
        self.assertEqual(g.converted('WEIGHTS_0',[[-1.,0.,0.,0.]]),[[1.,0.,0.,0.]])

    def test_morph_deltas_are_not_normalised(self):
        self.assertEqual(g.converted('NORMAL',[[2.,0.,0.]],morph=True),[[-2.,0.,0.]])

    def test_raw_values(self):
        self.assertEqual(g.converted('NORMAL',[[2.,0.,0.]],normalise=False),[[-2.,0.,0.]])
