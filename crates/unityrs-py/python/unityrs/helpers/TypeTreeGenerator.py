# Ported from UnityPy 1.25.2 (MIT): helpers/TypeTreeGenerator.py:1-78. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
import os
from typing import Dict, List, Tuple

from .TypeTreeNode import TypeTreeNode
from .._output import exact_string

try:
    from TypeTreeGeneratorAPI import TypeTreeGenerator as TypeTreeGeneratorBase  # pyright: ignore[reportAssignmentType]
except ImportError:

    class TypeTreeGeneratorBase:
        def __init__(self, unity_version: str):
            raise ImportError("TypeTreeGeneratorAPI isn't installed!")

        def load_dll(self, dll: bytes): ...
        def load_il2cpp(self, il2cpp: bytes, metadata: bytes): ...
        def get_nodes_as_json(self, assembly: str, fullname: str) -> str: ...
        def get_nodes(self, assembly: str, fullname: str) -> List[TypeTreeNode]: ...


class TypeTreeGenerator(TypeTreeGeneratorBase):
    cache: Dict[Tuple[str, str], TypeTreeNode]

    def __init__(self, unity_version: str, *args, backend=None, limits=None, budget=None, **kwargs):
        # Explicit backends support callers with an already configured optional
        # API. Generated nodes stay in this instance's in-memory cache.
        if backend is None:
            super().__init__(unity_version, *args, **kwargs)
        self._backend = backend
        self._limits, self._budget = limits, budget
        self.cache = {}

    def load_dll(self, dll: bytes):
        if self._backend is not None:
            return self._backend.load_dll(dll)
        return super().load_dll(dll)

    def load_il2cpp(self, il2cpp: bytes, metadata: bytes):
        if self._backend is not None:
            return self._backend.load_il2cpp(il2cpp, metadata)
        return super().load_il2cpp(il2cpp, metadata)

    def get_nodes(self, assembly: str, fullname: str):
        assembly, fullname = exact_string(assembly), exact_string(fullname)
        if self._backend is not None:
            return self._backend.get_nodes(assembly, fullname)
        return super().get_nodes(assembly, fullname)

    def load_local_game(self, root_dir: str):
        root_dir = exact_string(root_dir)
        root_files = os.listdir(root_dir)
        data_dir = os.path.join(root_dir, next(f for f in root_files if f.endswith("_Data")))
        if "GameAssembly.dll" in root_files:
            ga_fp = os.path.join(root_dir, "GameAssembly.dll")
            gm_fp = os.path.join(data_dir, "il2cpp_data", "Metadata", "global-metadata.dat")
            with open(ga_fp, "rb") as f:
                ga_raw = f.read()
            with open(gm_fp, "rb") as f:
                gm_raw = f.read()
            self.load_il2cpp(ga_raw, gm_raw)
        else:
            self.load_local_dll_folder(os.path.join(data_dir, "Managed"))

    def load_local_dll_folder(self, dll_dir: str):
        dll_dir = exact_string(dll_dir)
        for f in os.listdir(dll_dir):
            if not f.endswith(".dll"):
                continue
            fp = os.path.join(dll_dir, f)
            with open(fp, "rb") as f:
                data = f.read()
                self.load_dll(data)

    def get_nodes_up(self, assembly: str, fullname: str) -> TypeTreeNode:
        assembly, fullname = exact_string(assembly), exact_string(fullname)
        key = (assembly, fullname)
        if key in self.cache:
            return self.cache[key]

        if not assembly.endswith(".dll"):
            assembly = f"{assembly}.dll"

        base_nodes = self.get_nodes(assembly, fullname)

        from ._bounded_tree import guard_for, text_bound
        guard = guard_for(limits=getattr(self, '_limits', None), budget=getattr(self, '_budget', None))
        rows = []
        for base_node in base_nodes:
            size = text_bound(base_node.m_Type) + text_bound(base_node.m_Name)
            guard.step(base_node.m_Level + 1, 4096 + size * 4, 1 + size)
            rows.append(TypeTreeNode(
                base_node.m_Level, base_node.m_Type, base_node.m_Name, 0, 0,
                m_MetaFlag=base_node.m_MetaFlag,
            ))
        node = TypeTreeNode.from_list(rows, _guard=guard)

        self.cache[key] = node
        return node


__all__ = ("TypeTreeGenerator",)
