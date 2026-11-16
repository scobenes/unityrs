# Ported from UnityPy 1.25.2 (MIT): cli/update_tpk.py:1-27. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Download only on explicit invocation, keeping generated classes in memory.

The destination is a user cache, or an explicitly supplied directory. No class
source or schema derived from the downloaded TPK is written to disk.
"""
import os
from io import BytesIO
from pathlib import Path
from urllib.request import urlopen
from zipfile import ZipFile

URL = 'https://nightly.link/AssetRipper/Tpk/workflows/type_tree_tpk/master/lzma_file.zip'
RESOURCE_PATH = str(Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache')) / 'unityrs')
_MAX_DOWNLOAD = 256 << 20


def generate_classes():
    # Import the caller's configured TPK using the same checked runtime pipeline
    # as classes.generated, then discard the generated source after compilation.
    import importlib
    from .. import classes
    from ..classes import _generator, generated
    importlib.reload(_generator)
    previous = [name for name, value in vars(generated).items()
                if isinstance(value, type) and value.__module__ == generated.__name__]
    for name in previous:
        delattr(generated, name)
    importlib.reload(generated)
    for name in previous:
        if not hasattr(generated, name):
            vars(classes).pop(name, None)
    for name in vars(generated):
        if name.startswith('_'):
            continue
        setattr(classes, name, getattr(generated, name))
    from ..classes._exports import install as install_exports
    from ..classes._images import install as install_images
    from ..classes._gameobject import install as install_gameobject
    install_exports(vars(classes))
    install_images(vars(classes))
    install_gameobject(vars(classes))
    from ..classes import ClassIDTypeToClassMap
    importlib.reload(ClassIDTypeToClassMap)


def update_tpk(*, url=None, resource_path=None):
    from .. import config
    print('Updating TPK file...')
    print('\tDownloading...')
    with urlopen(URL if url is None else url) as response:
        if response.status != 200:
            raise Exception(f'Failed to download TPK file: {response.status} {response.reason}')
        zip_data = response.read(_MAX_DOWNLOAD + 1)
        if len(zip_data) > _MAX_DOWNLOAD:
            raise ValueError('TPK download exceeds 256 MiB')
    print('\tExtracting...')
    destination = Path(RESOURCE_PATH if resource_path is None else resource_path)
    with ZipFile(BytesIO(zip_data)) as archive:
        entry = archive.getinfo('lzma.tpk')
        if entry.file_size > _MAX_DOWNLOAD:
            raise ValueError('TPK file exceeds 256 MiB')
        data = archive.read(entry)
    destination.mkdir(parents=True, exist_ok=True)
    target = destination / 'lzma.tpk'
    target.write_bytes(data)
    config.TPK = str(target)
    print('\tGenerating classes...')
    generate_classes()
    print('\tDone.')


__all__ = ['update_tpk']
