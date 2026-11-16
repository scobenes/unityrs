# Ported from UnityPy 1.25.2 (MIT): helpers/TypeTreeHelper.py:27-75, helpers/TypeTreeHelper.py:364-404. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
import re
from ..streams import EndianBinaryReader, EndianBinaryWriter

kAlignBytes = 0x4000


def _read_aligned_string(reader):
    # Preserve the owning file reader's decoding staging guard.
    return reader.read_aligned_string()


FUNCTION_READ_MAP = {
    "SInt8": EndianBinaryReader.read_byte,
    "UInt8": EndianBinaryReader.read_u_byte,
    "char": EndianBinaryReader.read_u_byte,
    "short": EndianBinaryReader.read_short,
    "SInt16": EndianBinaryReader.read_short,
    "unsigned short": EndianBinaryReader.read_u_short,
    "UInt16": EndianBinaryReader.read_u_short,
    "int": EndianBinaryReader.read_int,
    "SInt32": EndianBinaryReader.read_int,
    "unsigned int": EndianBinaryReader.read_u_int,
    "UInt32": EndianBinaryReader.read_u_int,
    "Type*": EndianBinaryReader.read_u_int,
    "long long": EndianBinaryReader.read_long,
    "SInt64": EndianBinaryReader.read_long,
    "unsigned long long": EndianBinaryReader.read_u_long,
    "UInt64": EndianBinaryReader.read_u_long,
    "FileSize": EndianBinaryReader.read_u_long,
    "float": EndianBinaryReader.read_float,
    "double": EndianBinaryReader.read_double,
    "bool": EndianBinaryReader.read_boolean,
    "string": _read_aligned_string,
    "TypelessData": EndianBinaryReader.read_byte_array,
}

FUNCTION_READ_MAP_ARRAY = {
    "SInt8": EndianBinaryReader.read_byte_array,
    "UInt8": EndianBinaryReader.read_u_byte_array,
    "char": EndianBinaryReader.read_u_byte_array,
    "short": EndianBinaryReader.read_short_array,
    "SInt16": EndianBinaryReader.read_short_array,
    "unsigned short": EndianBinaryReader.read_u_short_array,
    "UInt16": EndianBinaryReader.read_u_short_array,
    "int": EndianBinaryReader.read_int_array,
    "SInt32": EndianBinaryReader.read_int_array,
    "unsigned int": EndianBinaryReader.read_u_int_array,
    "UInt32": EndianBinaryReader.read_u_int_array,
    "Type*": EndianBinaryReader.read_u_int_array,
    "long long": EndianBinaryReader.read_long_array,
    "SInt64": EndianBinaryReader.read_long_array,
    "unsigned long long": EndianBinaryReader.read_u_long_array,
    "UInt64": EndianBinaryReader.read_u_long_array,
    "FileSize": EndianBinaryReader.read_u_long_array,
    "float": EndianBinaryReader.read_float_array,
    "double": EndianBinaryReader.read_double_array,
    "bool": EndianBinaryReader.read_boolean_array,
}
def metaflag_is_aligned(meta_flag: int | None) -> bool:
    return ((meta_flag or 0) & kAlignBytes) != 0


def clean_name(name: str) -> str:
    if name.startswith("(int&)"):
        name = name[6:]
    if name.endswith("?"):
        name = name[:-1]
    name = re.sub(r"[ \.:\-\[\]]", "_", name)
    if name in ["pass", "from"]:
        name += "_"
    if name[0].isdigit():
        name = f"x{name}"
    return name


FUNCTION_WRITE_MAP = {
    "SInt8": EndianBinaryWriter.write_byte,
    "UInt8": EndianBinaryWriter.write_u_byte,
    "char": EndianBinaryWriter.write_u_byte,
    "short": EndianBinaryWriter.write_short,
    "SInt16": EndianBinaryWriter.write_short,
    "unsigned short": EndianBinaryWriter.write_u_short,
    "UInt16": EndianBinaryWriter.write_u_short,
    "int": EndianBinaryWriter.write_int,
    "SInt32": EndianBinaryWriter.write_int,
    "unsigned int": EndianBinaryWriter.write_u_int,
    "UInt32": EndianBinaryWriter.write_u_int,
    "Type*": EndianBinaryWriter.write_u_int,
    "long long": EndianBinaryWriter.write_long,
    "SInt64": EndianBinaryWriter.write_long,
    "unsigned long long": EndianBinaryWriter.write_u_long,
    "UInt64": EndianBinaryWriter.write_u_long,
    "FileSize": EndianBinaryWriter.write_u_long,
    "float": EndianBinaryWriter.write_float,
    "double": EndianBinaryWriter.write_double,
    "bool": EndianBinaryWriter.write_boolean,
    "string": EndianBinaryWriter.write_aligned_string,
    "TypelessData": EndianBinaryWriter.write_byte_array,
}
