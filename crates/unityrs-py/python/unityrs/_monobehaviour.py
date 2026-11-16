# Ported from UnityPy 1.25.2 (MIT): UnityPy/files/ObjectReader.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""MonoBehaviour provider hooks (files/ObjectReader.py:325-350)."""


def parse_monobehaviour_head(self, mb_node=None):
    if mb_node is None:
        from .helpers.TypeTreeNode import TypeTreeNode
        rows, guard = self._native.monobehaviour_head_nodes()
        for row in rows:
            guard.step(0, 256, 1)
            row.pop('_native_children', None)
            row.pop('_native_node', None)
            row.pop('_native_list', None)
        mb_node = TypeTreeNode.from_list(rows, _guard=guard)
    return self.read_typetree(nodes=mb_node, wrap=True, check_read=False)


def generate_monobehaviour_node(self, mb_node=None):
    generator = self.assets_file.environment.typetree_generator
    if generator is None:
        raise ValueError('MonoBehaviour detected, but no typetree_generator set to the environment!')
    monobehaviour = self.parse_monobehaviour_head(mb_node)
    script = monobehaviour.m_Script.deref_parse_as_object()
    guard = self._native.tree_traversal()
    size = sum(str.__len__(value) if isinstance(value, str) else 256
               for value in (script.m_Namespace, script.m_ClassName))
    guard.step(0, 512 + size * 4, 1)
    fullname = f'{script.m_Namespace}.{script.m_ClassName}' if script.m_Namespace != '' else script.m_ClassName
    node = generator.get_nodes_up(script.m_AssemblyName, fullname)
    if node:
        return node
    raise ValueError(f'Failed to generate MonoBehaviour node for {fullname} of {script.m_AssemblyName}!')


def generated_node(self, node):
    # files/ObjectReader.py:285-307: generated results stay out of the base cache.
    if not self._has_embedded_nodes and node.m_Type == 'MonoBehaviour':
        try:
            return self.generate_monobehaviour_node(node)
        except ValueError:
            pass
    return node
