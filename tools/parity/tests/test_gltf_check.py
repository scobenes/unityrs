"""Checks independent of the Rust writer, with deliberately broken payloads."""
import struct
import unittest
from tools.parity import gltf_check as g


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


class Checks(unittest.TestCase):
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
