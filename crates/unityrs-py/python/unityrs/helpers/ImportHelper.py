# Ported from UnityPy 1.25.2 (MIT): helpers/ImportHelper.py:1-173. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
from __future__ import annotations

import io
import os
from gzip import GzipFile
from typing import BinaryIO, List, Optional, Tuple, Union

from .. import files
from ..enums import FileType
from ..streams import EndianBinaryReader
from .CompressionHelper import BROTLI_MAGIC, GZIP_MAGIC

FileSourceType = Union[str, bytes, bytearray, io.IOBase, EndianBinaryReader, BinaryIO]


def file_name_without_extension(file_name: str) -> str:
    return os.path.join(os.path.dirname(file_name), os.path.splitext(os.path.basename(file_name))[0])


def list_all_files(directory: str) -> List[str]:
    return [
        val
        for sublist in [
            [os.path.join(dir_path, filename) for filename in filenames]
            for (dir_path, dirnames, filenames) in os.walk(directory)
            if ".git" not in dir_path
        ]
        for val in sublist
    ]


def find_all_files(directory: str, search_str: str) -> List[str]:
    return [
        val
        for sublist in [
            [os.path.join(dir_path, filename) for filename in filenames if search_str in filename]
            for (dir_path, dirnames, filenames) in os.walk(directory)
            if ".git" not in dir_path
        ]
        for val in sublist
    ]


def check_file_type(
    input_: FileSourceType,
) -> Tuple[FileType, EndianBinaryReader]:
    if isinstance(input_, str) and os.path.isfile(input_):
        reader = EndianBinaryReader(open(input_, "rb"))
    elif isinstance(input_, EndianBinaryReader):
        reader = input_
    else:
        reader = EndianBinaryReader(input_)

    if reader.Length < 20:
        return FileType.ResourceFile, reader

    signature = reader.read_string_to_null(20)

    reader.Position = 0
    if signature in [
        "UnityWeb",
        "UnityRaw",
        "\xfa\xfa\xfa\xfa\xfa\xfa\xfa\xfa",
        "UnityFS",
    ]:
        return FileType.BundleFile, reader
    elif signature.startswith(("UnityWebData", "TuanjieWebData")):
        return FileType.WebFile, reader
    elif signature == "PK\x03\x04":
        return FileType.ZIP, reader
    else:
        if reader.Length < 128:
            return FileType.ResourceFile, reader

        magic = bytes(reader.read_bytes(2))
        reader.Position = 0
        if GZIP_MAGIC == magic:
            g_stream = GzipFile(fileobj=reader)
            g_reader = EndianBinaryReader(g_stream, endian="<")
            signature = g_reader.read_string_to_null(20)
            g_stream.close()
            if signature.startswith(("UnityWebData", "TuanjieWebData")):
                return FileType.WebFile, reader
        reader.Position = 0x20
        magic = bytes(reader.read_bytes(6))
        reader.Position = 0
        if BROTLI_MAGIC == magic:
            return FileType.WebFile, reader

        # check if AssetsFile
        old_endian = reader.endian
        # read as if assetsfile and check version
        # ReadHeader
        reader.Position = 0
        metadata_size = reader.read_u_int()
        file_size = reader.read_u_int()
        version = reader.read_u_int()
        data_offset = reader.read_u_int()

        if version >= 22:
            raw_endian = reader.read_u_byte()
            _endian = ">" if raw_endian else "<"
            _reserved = reader.read_bytes(3)
            metadata_size = reader.read_u_int()
            file_size = reader.read_long()
            data_offset = reader.read_long()
            _unknown = reader.read_long()  # unknown

        # reset
        reader.endian = old_endian
        reader.Position = 0
        # check info
        if any(
            (
                version < 0,
                version > 100,
                *[x < 0 or x > reader.Length for x in [file_size, metadata_size, version, data_offset]],
                file_size < metadata_size,
                file_size < data_offset,
            )
        ):
            return FileType.ResourceFile, reader
        else:
            return FileType.AssetsFile, reader


def parse_file(
    reader: EndianBinaryReader,
    parent: files.File,
    name: str,
    typ: Optional[FileType] = None,
    is_dependency: bool = False,
) -> Union[files.File, EndianBinaryReader]:
    if typ is None:
        typ, _ = check_file_type(reader)
    try:
        if typ == FileType.AssetsFile:
            if name.endswith((".resS", ".resource", ".config", ".xml", ".dat")):
                return reader
        elif typ not in (FileType.BundleFile, FileType.WebFile):
            return reader
        if reader.Position != 0:
            raise NotImplementedError("parse_file nonzero parser position is not exposed")
        if typ in (FileType.AssetsFile, FileType.BundleFile) and reader.endian != ">":
            raise NotImplementedError("parse_file non-default parser endian is not exposed")
        from .. import Environment, config
        from .._native import TreeTraversal
        from ..streams.EndianBinaryReader import EndianBinaryReader_Streamable
        environment = getattr(parent, "environment", parent) if parent else None
        owner = environment if environment is not None else Environment()
        with owner._lock:
            stream = isinstance(reader, EndianBinaryReader_Streamable)
            stream = 2 if stream and isinstance(reader.stream, io.BytesIO) else int(stream)
            if isinstance(getattr(reader, "view", None), memoryview):
                data = reader.view
            else:
                # Keep reads bounded, including for streams whose declared
                # Length is false. _read_stream restores the caller's cursor.
                data, input_guard = owner._read_stream(reader)
            guard = TreeTraversal(owner._limits, owner._budget)
            text = name if isinstance(name, str) else ""
            guard.step(0, str.__len__(text) * 4 + 256, 0)
            native, position, endian, error = config._load(owner._native, owner._native.parse_bytes, data,
                                  text.encode("utf-8", "surrogateescape"), int(typ),
                                  stream, parent._native if isinstance(parent, files.File) else None)
            reader.Position = position
            if endian is not None:
                reader.endian = endian
            # Constructors register successful children in directory order,
            # even if a later child fails. Publish those before raising.
            if error is not None:
                owner._sync()
                raise error
            result = owner._wrap(native, parent, is_dependency)
            owner._sync()
            if isinstance(result, files.SerializedFile):
                result.reader = reader
                reader.endian = result.header.endian
            if environment is None:
                # Detached constructors expose no environment on any member.
                # Native handles already retain each file's context and meter.
                guard.step(0, 256, 1)
                pending = [result]
                while pending:
                    current = pending.pop()
                    current.environment = None
                    if isinstance(current, (files.BundleFile, files.WebFile)):
                        for child in current.files.values():
                            guard.step(0, 256, 1)
                            if isinstance(child, files.File):
                                pending.append(child)
            return result
    except Exception as error:
        reader.seek(0)
        print(f"Error parsing file {name!r} as {typ}: {error}")
        raise


def find_sensitive_path(dir: str, insensitive_path: str) -> Union[str, None]:
    parts = os.path.split(insensitive_path.strip(os.path.sep))

    sensitive_path = dir
    for part in parts:
        part_lower = part.lower()
        part = next(
            (name for name in os.listdir(sensitive_path) if name.lower() == part_lower),
            None,
        )
        if part is None:
            return None
        sensitive_path = os.path.join(sensitive_path, part)

    return sensitive_path
