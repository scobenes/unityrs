"""Y3 mutable tree public observations, replayed against both runtime modules.

All node data is synthetic. Common strings remain runtime-only provider data.
"""
import importlib
import io
import struct


class Reader:
    def __init__(self, data, endian="<"):
        self.data = data
        self.endian = endian
        self.Position = 0

    def read(self, size=-1):
        start = self.Position
        end = len(self.data) if size == -1 else start + size
        self.Position = end
        return self.data[start:end]

    def read_int(self):
        return struct.unpack(self.endian + "i", self.read(4))[0]

    def read_string_to_null(self):
        end = self.data.find(b"\0", self.Position)
        if end == -1:
            raise Exception("String not terminated")
        value = self.data[self.Position:end].decode("utf-8", "surrogateescape")
        self.Position = end + 1
        return value


class Writer:
    def __init__(self, endian="<"):
        self.endian = endian
        self._stream = io.BytesIO()

    @property
    def Position(self):
        return self._stream.tell()

    @property
    def bytes(self):
        return self._stream.getvalue()

    def write(self, data):
        return self._stream.write(data)

    def write_int(self, value):
        self.write(struct.pack(self.endian + "i", value))

    def write_string_to_null(self, value):
        self.write(value.encode("utf-8", "surrogateescape") + b"\0")


def observe(module):
    helper = importlib.import_module(module.__name__ + ".helpers.TypeTreeNode")
    Node = helper.TypeTreeNode
    helper.NAME_PEEK_NODE_CACHE.clear()
    results = {}

    def attempt(name, fn):
        try:
            results[name] = fn()
        except Exception as error:
            # Python includes the defining package in **kwargs and overflow errors.
            message = str(error).replace(module.__name__ + ".", "PACKAGE.")
            results[name] = {"error": type(error).__name__, "message": message}

    def row(node):
        return {key: value for key, value in node.to_dict().items() if key != "m_Children"}

    def tree(node):
        return {"nodes": [row(n) for n in node.traverse()], "repr": repr(node),
                "structure": node.dump_structure(), "custom_indent": node.dump_structure("|")}

    def make():
        return Node.from_list([
            dict(m_Level=0, m_Type="Synthetic", m_Name="Base", m_ByteSize=-1, m_Version=2,
                 m_TypeFlags=0, m_VariableCount=7, m_Index=0, m_MetaFlag=0, m_RefTypeHash=19),
            dict(m_Level=1, m_Type="int", m_Name="a.b", m_ByteSize=4, m_Version=1,
                 m_TypeFlags=1, m_VariableCount=8, m_Index=1, m_MetaFlag=0, m_RefTypeHash=20),
            dict(m_Level=1, m_Type="string", m_Name="m_Name", m_ByteSize=-1, m_Version=1,
                 m_TypeFlags=0, m_VariableCount=9, m_Index=2, m_MetaFlag=16384, m_RefTypeHash=21),
            dict(m_Level=2, m_Type="UInt8", m_Name="data", m_ByteSize=1, m_Version=1,
                 m_TypeFlags=0, m_VariableCount=10, m_Index=3, m_MetaFlag=0, m_RefTypeHash=22),
        ])

    attempt("tree", lambda: tree(make()))
    attempt("minimal", lambda: tree(Node(0, "int", "a", 4, 1)))
    attempt("defaults", lambda: tree(Node.from_list([
        {"m_Level": 0, "m_Type": "root", "m_Name": "r"},
        {"m_Level": 4, "m_Type": "int", "m_Name": "child"}])))
    for label, nodes in [
        ("empty", []), ("missing_level", [{"m_Type": "a", "m_Name": "b"}]),
        ("later_default_duplicate", [dict(m_Level=0, m_Type="r", m_Name="b"),
                                      dict(m_Level=1, m_Type="int", m_Name="x", m_ByteSize=4)]),
        ("later_missing_size", [dict(m_Level=0, m_Type="r", m_Name="b", m_ByteSize=0, m_Version=0),
                                dict(m_Level=1, m_Type="int", m_Name="x", m_Version=0)]),
        ("negative_level", [dict(m_Level=-2, m_Type="r", m_Name="b")]),
        ("mixed", [dict(m_Level=0, m_Type="r", m_Name="b"), 5]),
    ]:
        attempt("from_list_" + label, lambda nodes=nodes: tree(Node.from_list(nodes)))

    def identities():
        root = make()
        children = root.m_Children
        records = root.to_dict_list()
        nodes = list(root.traverse())
        same_dict_children = records[0]["m_Children"] is children
        linked = Node.from_list(nodes)
        return {"root": root is linked, "dict_children": same_dict_children,
                "child_objects": records[0]["m_Children"][0] is nodes[1],
                "order": [n.m_Name for n in linked.traverse()],
                "duplicates": children[0] is children[2],
                "nested_duplicates": nodes[2].m_Children[0] is nodes[2].m_Children[1]}

    attempt("identities", identities)

    def fields():
        node = Node(0, "int", "(int&)a.b?", 4, 1, None)
        before = node._clean_name
        node.m_Name = "changed"
        node.m_Level = "untyped mutation"
        node.m_MetaFlag = "flag"
        node.extra = 3
        return {"clean_before": before, "clean_after": node._clean_name,
                "dict": node.__dict__, "repr": repr(node), "row": row(node)}

    attempt("mutable_fields", fields)
    for args, kwargs, label in [
        ((), {}, "missing"), ((0,), {}, "missing_type"),
        ((0, "t", 1, 4, 1), {}, "name_type"), ((0, "t", "n", 4, 1), {"m_MetaFlag": "x"}, "meta_type"),
        ((0, "t", "n", 4, 1), {"extra": 1}, "unknown"),
        ((0, "t", "n", 4, 1), {"m_Level": 0}, "duplicate"),
        ((0, "t", "n", 4, 1, [], 0, None, None, None, None, 0), {}, "many"),
        ((False, "t", "n", True, True), {}, "bools"),
        ((0, "t", "n", 4, 1, ()), {}, "tuple_children"),
        ((0, "t", "?", 4, 1), {}, "empty_clean"),
        (("bad", "t", "n", 4, 1), {"extra": 0}, "required_before_unknown"),
        ((), dict(m_Level=0, m_Type="t", m_Name="n", m_ByteSize=0, m_Version=0,
                  m_Children=[], m_TypeFlags=0, m_VariableCount=0, m_Index=0,
                  m_MetaFlag=0, m_RefTypeHash=0, extra=0), "many_keywords"),
        ((0, "t", "n", 4, 1), {"extra": 0, "m_Level": 0}, "duplicate_before_unknown"),
    ]:
        attempt("constructor_" + label, lambda args=args, kwargs=kwargs: repr(Node(*args, **kwargs)))
    for name in ["", "?", "(int&)", "(int&)?", "(int&)1 a.b:c-d[e]?", "pass", "from", "1a", "²a", "a\0b"]:
        attempt("clean_" + name, lambda name=name: helper.clean_name(name))
        attempt("node_clean_" + name, lambda name=name: Node(0, "t", name, 0, 0)._clean_name)
    root = make()
    for value, label in [(5, "int"), (None, "none")]:
        attempt("equal_" + label, lambda value=value: root == value)
        attempt("unequal_" + label, lambda value=value: root != value)
    attempt("equal_tree", lambda: make() == make())
    attempt("hash", lambda: hash(root))

    def peek():
        root = make()
        result = root.get_name_peek_node()
        other = Node(0, root.m_Type, root.m_Name, 1, root.m_Version, [])
        no_name = Node(0, "no_name", "cache_none", 0, 0)
        initial = no_name.get_name_peek_node()
        no_name.m_Children.append(Node(1, "string", "name", -1, 1))
        return {"name": result[1], "nodes": [row(n) for n in result[0].traverse()],
                "same_child": result[0].m_Children[0] is root.m_Children[0],
                "different_list": result[0].m_Children is not root.m_Children,
                "same_key_cache": other.get_name_peek_node() is result,
                "none_cached": initial is None and no_name.get_name_peek_node() is None}

    attempt("peek", peek)
    for version in [2, 3, 11]:
        for endian in ["<", ">"]:
            def old(version=version, endian=endian):
                writer = Writer(endian)
                make().dump(writer, version)
                reader = Reader(writer.bytes, endian)
                try:
                    parsed = tree(Node.parse(reader, version))
                except Exception as error:
                    parsed = {"error": type(error).__name__, "message": str(error)}
                return {"bytes": writer.bytes.hex(), "parsed": parsed, "position": reader.Position}
            attempt(f"old_{version}_{endian}", old)
    for version in [18, 19]:
        for endian in ["<", ">"]:
            def blob(version=version, endian=endian):
                writer = Writer(endian)
                make().dump_blob(writer, version)
                reader = Reader(writer.bytes, endian)
                return {"bytes": writer.bytes.hex(), "parsed": tree(Node.parse_blob(reader, version)),
                        "position": reader.Position}
            attempt(f"blob_{version}_{endian}", blob)
    for version in [18, 19]:
        attempt(f"missing_optional_blob_{version}", lambda version=version: Node(0, "t", "n", 0, 0).dump_blob(Writer(), version))
    for label, data in [
        ("empty", b""), ("zero_count", struct.pack("<ii", 0, 0)),
        ("negative_count", struct.pack("<ii", -1, 0) + bytes(25)),
        ("short_row", struct.pack("<ii", 1, 0) + bytes(23)),
        ("null_missing", struct.pack("<iihBBIIiii", 1, 1, 1, 0, 0, 0, 0, 4, 0, 0) + b"x"),
        ("missing_common", struct.pack("<iihBBIIiii", 1, 0, 1, 0, 0, 0x80010000, 0x80010001, 4, 0, 0)),
        ("negative_buffer", struct.pack("<iihBBIIiii", 1, -1, 1, 0, 0, 0, 2, 4, 0, 0) + b"t\0n\0"),
    ]:
        attempt("parse_blob_" + label, lambda data=data: tree(Node.parse_blob(Reader(data), 18)))
    for version in [2, 3, 11]:
        attempt("parse_old_short_" + str(version), lambda version=version: Node.parse(Reader(b"t\0n\0"), version))
    # Observe lengths/errors only; no dump or provider-derived tree is saved.
    for label, buffer, offset in [
        ("long", b"x" * 40000 + b"\0", 0),
        ("window_edge", b"x" * 32766 + b"\0", 0),
        ("window_unterminated", b"x" * 32767, 0),
        ("past_end", b"x\0", 10),
    ]:
        def window(buffer=buffer, offset=offset):
            data = struct.pack("<iihBBIIiii", 1, len(buffer), 1, 0, 0,
                               offset, offset, 4, 0, 0) + buffer
            node = Node.parse_blob(Reader(data), 18)
            return [len(node.m_Type), len(node.m_Name)]
        attempt("parse_blob_window_" + label, window)
    def common_cache():
        first = helper.get_common_strings()
        again = helper.get_common_strings()
        zero = helper.get_common_strings(0)
        return {"same_cache": first is again, "zero_distinct_key": first is not zero}

    attempt("common_strings", common_cache)
    helper.NAME_PEEK_NODE_CACHE.clear()
    return results


def observe_mutation(module):
    """Boost caches its type and alignment; Python reads their current values."""
    from binding_cases import S, typed
    Node = importlib.import_module(module.__name__ + ".helpers.TypeTreeNode").TypeTreeNode
    results = {}
    for byte_size in (None, 0):
        for mutation in ("type", "align_removed", "align_added", "huge_metadata"):
            initially_aligned = mutation in ("type", "align_removed")
            payload = (b"\x01\0\0\0" + struct.pack("<i", 7) if initially_aligned else
                       b"\x01" + struct.pack("<i", 7) + b"\xaa\xbb\xcc")
            source = S.Node("TextAsset", "Base", [S.Node("bool", "first"), S.Node("int", "tail")])
            data = S.serialized_file([S.Type(49, source)], [S.Obj(1, 0, payload, byte_size=byte_size)])
            obj = module.Environment(data).objects[0]
            first = Node(1, "bool", "first", 1, 1, m_MetaFlag=0x4000 if initially_aligned else 0)
            second = Node(1, "int", "tail", 4, 1)
            root = Node(0, "TextAsset", "Base", -1, 1, [first, second])
            first.m_Name = "renamed"
            first._clean_name = "ignored_in_dict_mode"
            if mutation == "type":
                first.m_Type = "int"
                first.m_MetaFlag = 0
            elif mutation == "align_removed":
                first.m_MetaFlag = 0
            elif mutation == "align_added":
                first.m_MetaFlag = 0x4000
            else:
                for node in (root, first, second):
                    node.m_Level = 2**100
                    node.m_ByteSize = -(2**100)
                    node.m_Version = 2**100
                    node.m_TypeFlags = 2**100
                    node.m_VariableCount = 2**100
                    node.m_Index = -(2**100)
                    node.m_RefTypeHash = 2**100
                    node.m_MetaFlag = 2**100
            key = f"{byte_size}_{mutation}"
            try:
                results[key] = typed(obj.read_typetree(root, check_read=False))
            except Exception as error:
                results[key] = {"error": type(error).__name__, "message": str(error)}
    return results
