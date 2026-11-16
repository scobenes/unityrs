# Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/ContainerHelper.py. See THIRD_PARTY_LICENSES.
"""Multidict container compatibility (helpers/ContainerHelper.py:11-85)."""

from reprlib import recursive_repr


class ContainerHelper:
    __slots__ = ("container", "container_dict", "path_dict", "_preload_table", "__weakref__")
    __match_args__ = ("container", "container_dict", "path_dict", "_preload_table")

    def __init__(self, container):
        preload_table = None
        if not isinstance(container, list):
            preload_table = container.m_PreloadTable
            container = container.m_Container
        self.container = container
        self.container_dict = {key: value.asset for key, value in container}
        self.path_dict = {value.asset.path_id: key for key, value in container}
        self._preload_table = preload_table

    def parse_preload_table(self, *, _guard=None):
        if self._preload_table is None:
            return
        from ._bounded_tree import guard_for
        guard = guard_for(guard=_guard)
        guard.step(0, 256, 1)
        for path, info in self.container:
            guard.step(0, 0, 1)
            start = info.preloadIndex
            size = info.preloadSize
            if start < 0 or size <= 0 or start + size > len(self._preload_table):
                continue
            for index in range(start, start + size):
                guard.step(0, 0, 1)
                pptr = self._preload_table[index]
                if not pptr:
                    continue
                try:
                    target = pptr.deref()
                except (FileNotFoundError, KeyError):
                    continue
                from ..files import SerializedFile
                target_file = target.assets_file
                container = target_file.container if isinstance(target_file, SerializedFile) else target_file._container
                paths = container.path_dict
                path_id = pptr.path_id
                if path_id not in paths:
                    guard.step(0, 256, 1)
                    paths[path_id] = path
        self._preload_table = None

    def items(self):
        return ((key, value.asset) for key, value in self.container)

    def keys(self):
        return list({key for key, value in self.container})

    def values(self):
        return list({value.asset for key, value in self.container})

    def __getitem__(self, key):
        return self.container_dict[key]

    def __setitem__(self, key, value):
        raise NotImplementedError("Assigning to container is not allowed!")

    def __delitem__(self, key):
        raise NotImplementedError("Deleting from the container is not allowed!")

    def __iter__(self):
        return iter(self.keys())

    def __len__(self):
        return len(self.container)

    def __getattr__(self, name):
        return self.container_dict[name]

    def __str__(self):
        return f"{{{', '.join(f'{key}: {value}' for key, value in self.items())}}}"

    def __contains__(self, key):
        return key in self.container_dict

    def __eq__(self, other):
        # attrs' slots transform removes the source's __dict__ method and
        # synthesizes equality and representation for these four fields.
        if other.__class__ is not self.__class__:
            return NotImplemented
        return all(getattr(self, key) == getattr(other, key) for key in self.__match_args__)

    @recursive_repr("...")
    def __repr__(self):
        name = self.__class__.__qualname__.rsplit(">.", 1)[-1]
        return f"{name}(container={self.container!r}, container_dict={self.container_dict!r}, path_dict={self.path_dict!r}, _preload_table={self._preload_table!r})"
