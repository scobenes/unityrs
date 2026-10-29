//! Support for parsing compressed type tree dumps from [`AssetRipper/Tpk`](https://github.com/AssetRipper/Tpk)
//!
//! ```rust,no_run
//! use std::fs::File;
//! use rabex::tpk::TpkFile;
//! use rabex::objects::ClassId;
//! # use anyhow::Result;
//!
//! fn main() -> Result<()> {
//!     let mut tpk_file = File::open("lz4.tpk")?;
//!     let tpk_file = TpkFile::from_reader(&mut tpk_file)?;
//!     let tpk = tpk_file.as_type_tree()?.unwrap();
//!
//!     let version = "2023.2.18f1".parse().unwrap();
//!
//!     let ty = tpk.get_typetree_node(ClassId::GameObject, &version).unwrap();
//!     println!("{}", ty.dump());
//!
//!     Ok(())
//! }
//!
//! ```
//!
//! [`TpkTypeTreeBlob`] implements the [`TypeTreeProvider`](crate::typetree::TypeTreeProvider) crate, so with
//! the `embed-tpk` feature, using TPK to look up types is as easy as
//! ```rust
//! # use rabex::tpk::TpkTypeTreeBlob;
//! # use rabex::typetree::typetree_cache::TypeTreeCache;
//! # use rabex::typetree::TypeTreeProvider;
//! # use rabex::objects::ClassId;
//! # let unity_version = "2020.2.2f1".parse().unwrap();
//! let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
//! let node = tpk.get_typetree_node(ClassId::Transform, &unity_version);
//! ```

#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    InvalidMagic,
    UnsupportedVersion(u8),
    IO(std::io::Error),
    UTF8(std::string::FromUtf8Error),
    UnsupportedCompression(&'static str),
    Decompression(Box<dyn std::error::Error + Send + Sync + 'static>),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidMagic => write!(f, "Invalid TPK magic value"),
            Error::UnsupportedVersion(ver) => write!(f, "Unsupported version: {ver}"),
            Error::IO(e) => write!(f, "IO error: {e}"),
            Error::UnsupportedCompression(method) => {
                write!(f, "rabex feature for {method} support is not enabled")
            }
            Error::UTF8(e) => write!(f, "invalid utf8: {e}"),
            Error::Decompression(e) => write!(f, "failed to decompress: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::IO(e) => Some(e),
            Error::UTF8(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::IO(err)
    }
}
impl<T: TryFromPrimitive> From<TryFromPrimitiveError<T>> for Error {
    fn from(err: TryFromPrimitiveError<T>) -> Self {
        Error::IO(std::io::Error::other(format!(
            "No discriminant in enum `{name}` matches the value `{input:?}`",
            name = T::NAME,
            input = err.number,
        )))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// F3 (unityrs): malformed data that used to panic is an error.
fn invalid(msg: &'static str) -> Error {
    Error::IO(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
}

/// F3 (unityrs): a size read from the file as an `i32`. A negative one is
/// an error; a size never sizes an allocation before its bytes are read.
fn size(n: i32, what: &'static str) -> Result<usize> {
    usize::try_from(n).map_err(|_| invalid(what))
}

/// F3 (unityrs): a count UnityPy's `Tpk.py` reads with `range(count)`, which
/// reads nothing for a negative one. It never sizes an allocation: a huge
/// count fails when the bytes run out.
fn range_count(n: i32) -> usize {
    usize::try_from(n).unwrap_or(0)
}

/// F3 (unityrs): a count UnityPy's `Tpk.py` reads as a struct format
/// (`f"<{count}Q"`), which raises on a negative one.
fn struct_count(n: i32) -> Result<usize> {
    usize::try_from(n).map_err(|_| invalid("negative count"))
}

/// F3 (unityrs): the bounds of [`TpkTypeTreeBlob::get_typetree_node_for_class`]'s
/// walk. A node that lists itself or an ancestor reaches the depth.
const MAX_WALK_DEPTH: usize = 1024;
const MAX_WALK_ROWS: usize = 1 << 16;

/// F5 (unityrs): the most a decompression buffer reserves up front from the
/// size the file names; it grows past that with the decoder's output.
const MAX_PREALLOCATION: usize = 1 << 24;

use bitflags::bitflags;
use num_enum::{TryFromPrimitive, TryFromPrimitiveError};
use rustc_hash::FxHashMap;

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::Read;

use byteorder::{LittleEndian, ReadBytesExt};

use crate::objects::ClassId;
use crate::typetree::TypeTreeNode;
use crate::unity_version::{UnityVersion, UnityVersionType};

/// The TPK container type, contained (possibly compressed) data for the inner format.
#[derive(Debug, Clone)]
pub struct TpkFile {
    pub compression_type: TpkCompressionType,
    pub data_type: TpkDataType,
    pub compressed_size: i32,
    pub uncompressed_size: i32,
    pub compressed_bytes: Vec<u8>,
}

impl TpkFile {
    pub fn from_reader<T: std::io::Read>(reader: &mut T) -> Result<TpkFile> {
        let magic = reader.read_u32::<LittleEndian>()?;
        if u32::to_le_bytes(magic) != *b"TPK*" {
            return Err(Error::InvalidMagic);
        }

        let version_number = reader.read_u8()?;
        if version_number != 1 {
            return Err(Error::UnsupportedVersion(version_number));
        }

        let compression_type = TpkCompressionType::try_from(reader.read_u8()?)?;
        let data_type = TpkDataType::try_from(reader.read_u8()?)?;
        reader.read_u8()?;
        reader.read_u32::<LittleEndian>()?;
        let compressed_size = reader.read_i32::<LittleEndian>()?;
        let decompressed_size = reader.read_i32::<LittleEndian>()?;
        // F3 (unityrs): the buffer grows with the bytes read, never to a
        // size the file names before they exist.
        let len = size(compressed_size, "negative compressed size")?;
        let mut compressed_bytes = Vec::new();
        reader.take(len as u64).read_to_end(&mut compressed_bytes)?;
        if compressed_bytes.len() != len {
            return Err(Error::IO(std::io::ErrorKind::UnexpectedEof.into()));
        }

        Ok(TpkFile {
            compression_type,
            data_type,
            compressed_size,
            uncompressed_size: decompressed_size,
            compressed_bytes,
        })
    }

    pub fn decompress(&self) -> Result<Cow<'_, [u8]>> {
        Ok(match self.compression_type {
            TpkCompressionType::None => Cow::Borrowed(&self.compressed_bytes),

            #[cfg(not(feature = "compression-lz4"))]
            TpkCompressionType::Lz4 => return Err(Error::UnsupportedCompression("lz4")),
            #[cfg(feature = "compression-lz4")]
            TpkCompressionType::Lz4 => {
                // F3 (unityrs): an LZ4 block expands at most 255 times, so a
                // larger size cannot be met and is refused before the output
                // buffer is allocated at it.
                let len = size(self.uncompressed_size, "negative uncompressed size")?;
                if len
                    > self
                        .compressed_bytes
                        .len()
                        .saturating_mul(255)
                        .saturating_add(255)
                {
                    return Err(invalid(
                        "uncompressed size past what the LZ4 block can hold",
                    ));
                }
                lz4_flex::block::decompress(&self.compressed_bytes, len)
                    .map_err(|e| Error::Decompression(Box::new(e)))?
                    .into()
            }
            #[cfg(not(feature = "compression-lzma"))]
            TpkCompressionType::Lzma => return Err(Error::UnsupportedCompression("lzma")),
            #[cfg(feature = "compression-lzma")]
            TpkCompressionType::Lzma => {
                // F5 (unityrs): the decoder appends to its `Write` target, so
                // the buffer starts empty. A pre-zeroed one put
                // `uncompressed_size` zeros before the payload, which read as
                // a blob with no versions and no classes, without an error.
                let len = size(self.uncompressed_size, "negative uncompressed size")?;
                let mut uncompressed = Vec::with_capacity(len.min(MAX_PREALLOCATION));
                lzma_rs::lzma_decompress_with_options(
                    &mut self.compressed_bytes.as_slice(),
                    &mut uncompressed,
                    &lzma_rs::decompress::Options {
                        unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(Some(
                            self.uncompressed_size as u64,
                        )),
                        memlimit: None,
                        allow_incomplete: false,
                    },
                )
                .map_err(|e| Error::Decompression(Box::new(e)))?;
                Cow::Owned(uncompressed)
            }
            #[cfg(not(feature = "tpk-compression-brotli"))]
            TpkCompressionType::Brotli => return Err(Error::UnsupportedCompression("brotli")),
            #[cfg(feature = "tpk-compression-brotli")]
            TpkCompressionType::Brotli => {
                // F5 (unityrs): as for LZMA, an empty buffer.
                let len = size(self.uncompressed_size, "negative uncompressed size")?;
                let mut uncompressed = Vec::with_capacity(len.min(MAX_PREALLOCATION));
                brotli::BrotliDecompress(&mut self.compressed_bytes.as_slice(), &mut uncompressed)?;
                Cow::Owned(uncompressed)
            }
        })
    }

    /// Reads the inner data in case of [`TpkDataType::TypeTreeInformation`]
    pub fn as_type_tree(&self) -> Result<Option<TpkTypeTreeBlob>> {
        match self.data_type {
            TpkDataType::TypeTreeInformation => {
                let data = self.decompress()?;
                let type_tree = TpkTypeTreeBlob::from_reader(&mut data.as_ref())?;

                Ok(Some(type_tree))
            }
            _ => Ok(None),
        }
    }
}

impl TpkTypeTreeBlob {
    /// A type tree blob bundled with the crate. Might not always contain the newest version,
    /// pleas open an issue if you need a more modern release.
    #[cfg(feature = "embed-tpk")]
    pub fn embedded() -> Self {
        let bytes = include_bytes!("../resources/lz4.tpk");
        let tpk_file = TpkFile::from_reader(&mut bytes.as_slice()).unwrap();
        tpk_file.as_type_tree().unwrap().unwrap()
    }
}

#[repr(u8)]
#[derive(Debug, Clone, TryFromPrimitive)]
#[non_exhaustive]
pub enum TpkCompressionType {
    None,
    Lz4,
    Lzma,
    Brotli,
}

#[repr(u8)]
#[derive(Debug, Clone, TryFromPrimitive)]
pub enum TpkDataType {
    /// Information about the structure of Unity assets
    TypeTreeInformation = 0,
    /// A collection of blobs
    Collection = 1,
    /// A file system archive
    FileSystem = 2,
    /// Custom json data
    Json = 3,
    /// Lists of reference assemblies in the editor
    ReferenceAssemblies = 4,
    /// Lists of default Unity assets and their export ids
    EngineAssets = 5,
}

/// Size-Optimized store of unity type trees across multiple unity versions.
#[derive(Debug, Clone)]
pub struct TpkTypeTreeBlob {
    pub creation_time: i64,
    pub versions: Vec<UnityVersion>,
    pub class_information: FxHashMap<ClassId, Vec<(UnityVersion, Option<UnityClass>)>>,
    pub common_string: TpkCommonString,
    pub node_buffer: Vec<TpkUnityNode>,
    pub string_buffer: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TpkClassInformation {
    pub id: i32,
    pub classes: Vec<(UnityVersion, Option<UnityClass>)>,
}
impl TpkClassInformation {
    pub fn read<R: Read>(reader: &mut R) -> Result<TpkClassInformation> {
        let id = reader.read_i32::<LittleEndian>()?;
        let count = range_count(reader.read_i32::<LittleEndian>()?);
        let mut classes = Vec::new();
        for _ in 0..count {
            let version = UnityVersion::read_tpk_encoding(reader)?;
            let has_class_data = reader.read_u8()? != 0;
            let class = has_class_data
                .then(|| UnityClass::read(reader))
                .transpose()?;
            classes.push((version, class))
        }
        Ok(TpkClassInformation { id, classes })
    }
}

bitflags! {
#[derive(Clone, Debug)]
pub struct TpkUnityClassFlags : u8 {
    /// None of the flags apply to this class
    const NONE = 0;
    /// Is the class abstract?
    const IS_ABSTRACT = 1;
    /// Is the class sealed? Not necessarily accurate.
    const IS_SEALED = 2;
    /// Does the class only appear in the editor?
    const IS_EDITOR_ONLY = 4;
    /// Does the class only appear in game files? Not currently used
    const IS_RELEASE_ONLY = 8;
    /// Is the class stripped?
    const IS_STRIPPED = 16;
    /// Not currently used
    const RESERVED = 32;
    /// Does the class have an editor root node?
    const HAS_EDITOR_ROOT_NODE = 64;
    /// Does the class have a release root node?
    const HAS_RELEASE_ROOT_NODE = 128;
}
}

#[derive(Debug, Clone)]
pub struct UnityClass {
    pub name: u16,
    pub base: u16,
    pub flags: TpkUnityClassFlags,
    pub editor_root_node: Option<u16>,
    pub release_root_node: Option<u16>,
}

impl UnityClass {
    pub fn read<R: Read>(reader: &mut R) -> Result<UnityClass> {
        let name = reader.read_u16::<LittleEndian>()?;
        let base = reader.read_u16::<LittleEndian>()?;
        // F3 (unityrs): every byte is a valid set of flags; no unwrap.
        let flags = TpkUnityClassFlags::from_bits_retain(reader.read_u8()?);
        let editor_root_node = flags
            .contains(TpkUnityClassFlags::HAS_EDITOR_ROOT_NODE)
            .then(|| reader.read_u16::<LittleEndian>())
            .transpose()?;
        // F6 (unityrs): the release root is read under its own flag, as
        // UnityPy reads each root (`Tpk.py:302-305`); testing the editor
        // flag twice misread every class with one root and shifted every
        // read after it.
        let release_root_node = flags
            .contains(TpkUnityClassFlags::HAS_RELEASE_ROOT_NODE)
            .then(|| reader.read_u16::<LittleEndian>())
            .transpose()?;
        Ok(UnityClass {
            name,
            base,
            flags,
            editor_root_node,
            release_root_node,
        })
    }
}

#[derive(Debug, Clone)]
pub struct TpkCommonString {
    pub version_information: Vec<(UnityVersion, u8)>,
    pub string_buffer_indices: Vec<u16>,
}
impl TpkCommonString {
    pub fn read<R: Read>(reader: &mut R) -> Result<TpkCommonString> {
        let version_count = range_count(reader.read_i32::<LittleEndian>()?);
        let mut version_information = Vec::new();
        for _ in 0..version_count {
            let version = UnityVersion::read_tpk_encoding(reader)?;
            let string_count = reader.read_u8()?;
            version_information.push((version, string_count));
        }
        let indices_count = struct_count(reader.read_i32::<LittleEndian>()?)?;
        let mut string_buffer_indices = Vec::new();
        for _ in 0..indices_count {
            string_buffer_indices.push(reader.read_u16::<LittleEndian>()?);
        }

        Ok(TpkCommonString {
            version_information,
            string_buffer_indices,
        })
    }

    pub fn get_count(&self, target_version: &UnityVersion) -> Option<u8> {
        // F3 (unityrs): the forward scan, which stops at the first version
        // above the target, is UnityPy's `get_item_for_version`; an
        // assertion that a backward scan agrees panicked on a list out of
        // order.
        let mut ret = None;
        for (version, item) in &self.version_information {
            if target_version >= version {
                ret = Some(*item)
            } else {
                break;
            }
        }
        ret
    }
}

#[derive(Debug, Clone)]
pub struct TpkUnityNode {
    pub type_name: u16,
    pub name: u16,
    pub byte_size: i32,
    pub version: i16,
    pub type_flags: u8,
    pub meta_flag: u32,
    pub sub_nodes: Vec<u16>,
}
impl TpkUnityNode {
    pub fn read<R: Read>(reader: &mut R) -> Result<TpkUnityNode> {
        let type_name = reader.read_u16::<LittleEndian>()?;
        let name = reader.read_u16::<LittleEndian>()?;
        let byte_size = reader.read_i32::<LittleEndian>()?;
        let version = reader.read_i16::<LittleEndian>()?;
        let type_flags = reader.read_u8()?;
        let meta_flag = reader.read_u32::<LittleEndian>()?;
        let node_count = reader.read_u16::<LittleEndian>()?;
        let mut sub_nodes = Vec::with_capacity(node_count as usize);
        for _ in 0..node_count {
            sub_nodes.push(reader.read_u16::<LittleEndian>()?);
        }

        Ok(TpkUnityNode {
            type_name,
            name,
            byte_size,
            version,
            type_flags,
            meta_flag,
            sub_nodes,
        })
    }
}

impl UnityVersion {
    fn read_tpk_encoding<R: Read>(reader: &mut R) -> Result<UnityVersion> {
        let version = reader.read_u64::<LittleEndian>()?;
        let major = ((version >> 48) & 0xFFFF) as u16;
        let minor = ((version >> 32) & 0xFFFF) as u16;
        let build = ((version >> 16) & 0xFFFF) as u16;
        let typ = UnityVersionType::try_from(((version >> 8) & 0xFF) as u8)?;
        let type_number = (version & 0xFF) as u8;
        Ok(UnityVersion {
            major,
            minor,
            build,
            typ,
            build_number: type_number,
            trailing_data: String::new(),
        })
    }
}

impl TpkTypeTreeBlob {
    pub fn from_reader<R: Read>(reader: &mut R) -> Result<TpkTypeTreeBlob> {
        let creation_time = reader.read_i64::<LittleEndian>()?;

        // F3 (unityrs): counts are checked and never size an allocation.
        let version_count = struct_count(reader.read_i32::<LittleEndian>()?)?;
        let mut versions = Vec::new();
        for _ in 0..version_count {
            versions.push(UnityVersion::read_tpk_encoding(reader)?);
        }

        let class_count = range_count(reader.read_i32::<LittleEndian>()?);
        let mut class_information = FxHashMap::default();
        for _ in 0..class_count {
            let class = TpkClassInformation::read(reader)?;
            class_information.insert(ClassId(class.id), class.classes);
        }

        let common_string = TpkCommonString::read(reader)?;

        let node_count = range_count(reader.read_i32::<LittleEndian>()?);
        let mut node_buffer = Vec::new();
        for _ in 0..node_count {
            node_buffer.push(TpkUnityNode::read(reader)?);
        }
        let string_count = range_count(reader.read_i32::<LittleEndian>()?);
        let mut string_buffer = Vec::new();
        for _ in 0..string_count {
            string_buffer.push(read_7bit_encoded_string(reader)?);
        }

        Ok(TpkTypeTreeBlob {
            creation_time,
            versions,
            class_information,
            common_string,
            node_buffer,
            string_buffer,
        })
    }

    /// Look up and construct a [`TypeTreeNode`] for the class and unity version.
    /// If you call this method often, consider using [`TpkTypeTreeCache`](crate::typetree::typetree_cache::TypeTreeCache).
    pub fn get_typetree_node(
        &self,
        class_id: ClassId,
        target_version: &UnityVersion,
    ) -> Option<TypeTreeNode> {
        self.try_get_typetree_node(class_id, target_version)
            .ok()
            .flatten()
    }

    /// [`Self::get_typetree_node`], with a malformed node tree (a node or
    /// string id past its table, a node that lists itself or an ancestor)
    /// an error rather than `None`.
    pub fn try_get_typetree_node(
        &self,
        class_id: ClassId,
        target_version: &UnityVersion,
    ) -> Result<Option<TypeTreeNode>> {
        // F2 (unityrs): a class the blob does not hold has no tree, where
        // indexing the map panicked.
        let Some(version_classes) = self.class_information.get(&class_id) else {
            return Ok(None);
        };

        let Some(class) = version_classes
            .iter()
            .rev()
            .find_map(|(version, class)| (target_version >= version).then_some(class))
            .and_then(Option::as_ref)
        else {
            return Ok(None);
        };

        self.try_get_typetree_node_for_class(class, false)
    }

    pub fn get_typetree_node_for_class(
        &self,
        class: &UnityClass,
        editor: bool,
    ) -> Option<TypeTreeNode> {
        self.try_get_typetree_node_for_class(class, editor)
            .ok()
            .flatten()
    }

    /// [`Self::get_typetree_node_for_class`], with a malformed node tree an
    /// error rather than `None`.
    pub fn try_get_typetree_node_for_class(
        &self,
        class: &UnityClass,
        editor: bool,
    ) -> Result<Option<TypeTreeNode>> {
        let mut nodes = Vec::new();

        let root = match editor {
            false => class.release_root_node,
            true => class.editor_root_node,
        };
        let Some(root) = root else {
            return Ok(None);
        };

        self.walk_nodes(
            root,
            MAX_WALK_DEPTH,
            MAX_WALK_ROWS,
            |level, node, type_name, name| {
                nodes.push((
                    level,
                    TypeTreeNode {
                        m_Version: node.version as i32,
                        m_TypeFlags: node.type_flags as i32,
                        m_ByteSize: node.byte_size,
                        m_MetaFlag: Some(node.meta_flag as i32),
                        m_Type: type_name.to_owned(),
                        m_Name: name.to_owned(),
                        m_RefTypeHash: None,
                        m_VariableCount: None,
                        children: Vec::new(),
                    },
                ))
            },
        )?;

        reconstruct_tree(nodes).map(Some)
    }

    /// F3 (unityrs): UnityPy's node walk (`generate_node`, `Tpk.py:58-88`):
    /// from `root`, every node in pre-order with its level (its parent's
    /// plus one), passed to `row`. Node and string ids are bounds-checked,
    /// the level is a `usize`, and a walk that would reach a level of
    /// `max_depth` or more than `max_rows` rows stops with an error, so a
    /// node that lists itself or an ancestor ends instead of growing
    /// without end. The pending stack counts towards `max_rows`, since each
    /// entry on it becomes a row.
    fn walk_nodes(
        &self,
        root: u16,
        max_depth: usize,
        max_rows: usize,
        mut row: impl FnMut(usize, &TpkUnityNode, &str, &str),
    ) -> Result<()> {
        let string = |id: u16| {
            self.string_buffer
                .get(usize::from(id))
                .ok_or_else(|| invalid("TPK string id past the string buffer"))
        };
        let mut stack = VecDeque::new();
        stack.push_back((root, 0_usize));
        let mut rows = 0_usize;

        // adapted from https://github.com/K0lb3/UnityPy/blob/62bf31064bb36659661f024a770f6e55f3fbd3d3/UnityPy/helpers/Tpk.py#L55
        while let Some((node_id, level)) = stack.pop_front() {
            let node = self
                .node_buffer
                .get(usize::from(node_id))
                .ok_or_else(|| invalid("TPK node id past the node buffer"))?;
            row(level, node, string(node.type_name)?, string(node.name)?);
            rows += 1;
            if node.sub_nodes.is_empty() {
                continue;
            }
            if level + 1 >= max_depth {
                return Err(invalid(
                    "TPK node tree deeper than the walk allows (a node lists itself or an ancestor?)",
                ));
            }
            if rows + stack.len() + node.sub_nodes.len() > max_rows {
                return Err(invalid("TPK node tree has more rows than the walk allows"));
            }
            for &sub_node_id in node.sub_nodes.iter().rev() {
                stack.push_front((sub_node_id, level + 1));
            }
        }
        Ok(())
    }
}

fn read_7bit_encoded_int<R: Read>(reader: &mut R) -> Result<usize, std::io::Error> {
    let mut result = 0usize;
    let mut shift = 0;

    loop {
        let byte = reader.read_u8()?;
        result |= ((byte & 0x7F) as usize) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;

        if shift >= 35 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Too many bytes in 7-bit encoded int",
            ));
        }
    }

    Ok(result)
}

/// Reads a 7-bit-length-prefixed UTF-8 string from a stream
fn read_7bit_encoded_string<R: Read>(reader: &mut R) -> Result<String> {
    let len = read_7bit_encoded_int(reader)?;
    // F3 (unityrs): the length (up to 2^35 - 1) never sizes the buffer. It
    // is read through `take(len)` a chunk at a time, and the buffer grows
    // by exactly the bytes each read returns, so a length past the end of
    // the data costs no more than the bytes that are there.
    let mut source = reader.take(len as u64);
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = match source.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        buffer.reserve_exact(n);
        buffer.extend_from_slice(&chunk[..n]);
    }
    if buffer.len() != len {
        return Err(Error::IO(std::io::ErrorKind::UnexpectedEof.into()));
    }
    let string = String::from_utf8(buffer).map_err(Error::UTF8)?;
    Ok(string)
}

/// Given a flat list of TypeTreeNodes, reconstruct the tree in memory.
/// Note: This method assumes that the nodes are in the order of the
/// flattened representation of the tree, i.e.
/// level=0 idx=1
/// level=0 idx=2
///   level=1 idx=1
///   level=1 idx=2
///   level=1 idx=3
/// level=0 idx=3
/// level=0 idx=4
///
/// F3 (unityrs): the levels are `usize` (a `u8` wrapped past 255 levels), and
/// rows whose levels do not nest are an error rather than a panic.
fn reconstruct_tree(flat_nodes: Vec<(usize, TypeTreeNode)>) -> Result<TypeTreeNode> {
    fn access(mut node: &mut TypeTreeNode, mut depth: usize) -> Option<&mut TypeTreeNode> {
        while depth > 0 {
            node = node.children.last_mut()?;
            depth -= 1;
        }
        Some(node)
    }

    let mut flat_nodes = flat_nodes.into_iter();
    let (_, mut root) = flat_nodes
        .next()
        .ok_or_else(|| invalid("TPK node tree has no rows"))?;

    for (level, flat_node) in flat_nodes {
        level
            .checked_sub(1)
            .and_then(|depth| access(&mut root, depth))
            .ok_or_else(|| invalid("TPK node rows do not nest"))?
            .children
            .push(flat_node);
    }

    Ok(root)
}

#[cfg(all(test, feature = "embed-tpk"))]
mod commonstring_tests {
    use super::TpkTypeTreeBlob;
    use crate::commonstring::COMMONSTRING;
    use std::collections::BTreeMap;

    /// `src/commonstring.rs` is generated from the TPK via `just update-commonstring`. This
    /// guards against it drifting — a hand edit, or a TPK update without regenerating. The
    /// hardcoded read table must equal the full TPK-derived common string table.
    #[test]
    fn commonstring_matches_tpk() {
        let tpk = TpkTypeTreeBlob::embedded();
        let mut tpk_map = BTreeMap::new();
        let mut offset = 0u32;
        for &i in &tpk.common_string.string_buffer_indices {
            let s = tpk.string_buffer[i as usize].as_str();
            tpk_map.insert(offset, s);
            offset += s.len() as u32 + 1;
        }

        assert_eq!(COMMONSTRING.len(), tpk_map.len());
        for (off, s) in COMMONSTRING.iter() {
            assert_eq!(tpk_map.get(off).copied(), Some(*s), "offset {off}");
        }
    }
}

/// Regression tests for the unityrs patches (`UPSTREAM.md` of the vendored
/// crate), on TPKs built in code.
#[cfg(test)]
mod patch_tests {
    use super::*;

    /// A class entry as stored: its flags, then each root its flags name.
    #[derive(Clone)]
    pub(super) struct Class {
        pub flags: u8,
        pub editor_root: Option<u16>,
        pub release_root: Option<u16>,
    }

    impl Class {
        /// A class with a release root only, flagged as both (the flags
        /// upstream reads both roots under).
        pub fn both(root: u16) -> Self {
            Class {
                flags: 64 | 128,
                editor_root: Some(root),
                release_root: Some(root),
            }
        }
    }

    pub(super) struct Node {
        pub type_name: u16,
        pub name: u16,
        pub byte_size: i32,
        pub version: i16,
        pub type_flags: u8,
        pub meta_flag: u32,
        pub sub_nodes: Vec<u16>,
    }

    impl Node {
        pub fn new(type_name: u16, name: u16, sub_nodes: Vec<u16>) -> Self {
            Node {
                type_name,
                name,
                byte_size: -1,
                version: 1,
                type_flags: 0,
                meta_flag: 0x4000,
                sub_nodes,
            }
        }
    }

    /// A class id and its entries, each a version and a class or none.
    pub(super) type ClassEntries = (i32, Vec<(u64, Option<Class>)>);

    /// A type-tree blob, written as UnityPy's `Tpk.py` reads one.
    #[derive(Default)]
    pub(super) struct Blob {
        pub versions: Vec<u64>,
        pub classes: Vec<ClassEntries>,
        pub common: Vec<(u64, u8)>,
        pub common_indices: Vec<u16>,
        pub nodes: Vec<Node>,
        pub strings: Vec<Vec<u8>>,
    }

    /// Where each count of a written blob is, to set it to something else.
    #[derive(Default, Debug)]
    pub(super) struct Offsets {
        pub version_count: usize,
        pub class_count: usize,
        pub entry_count: usize,
        pub common_version_count: usize,
        pub common_indices_count: usize,
        pub node_count: usize,
        pub string_count: usize,
    }

    fn count(out: &mut Vec<u8>, n: usize) -> usize {
        let at = out.len();
        out.extend_from_slice(&i32::try_from(n).unwrap().to_le_bytes());
        at
    }

    pub(super) fn varint(out: &mut Vec<u8>, mut n: u64) {
        loop {
            let byte = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    impl Blob {
        /// Strings `s0`, `s1`, ... for `n` ids, and a 2.0.0 version list.
        pub fn with_strings(n: usize) -> Self {
            Blob {
                versions: vec![2 << 48],
                strings: (0..n).map(|i| format!("s{i}").into_bytes()).collect(),
                ..Blob::default()
            }
        }

        pub fn write(&self) -> (Vec<u8>, Offsets) {
            let mut out = Vec::new();
            let mut at = Offsets::default();
            out.extend_from_slice(&1_i64.to_le_bytes());
            at.version_count = count(&mut out, self.versions.len());
            for v in &self.versions {
                out.extend_from_slice(&v.to_le_bytes());
            }
            at.class_count = count(&mut out, self.classes.len());
            for (i, (id, entries)) in self.classes.iter().enumerate() {
                out.extend_from_slice(&id.to_le_bytes());
                let entry_count = count(&mut out, entries.len());
                if i == 0 {
                    at.entry_count = entry_count;
                }
                for (version, class) in entries {
                    out.extend_from_slice(&version.to_le_bytes());
                    match class {
                        None => out.push(0),
                        Some(c) => {
                            out.push(1);
                            out.extend_from_slice(&7_u16.to_le_bytes()); // name
                            out.extend_from_slice(&0_u16.to_le_bytes()); // base
                            out.push(c.flags);
                            for root in [c.editor_root, c.release_root].into_iter().flatten() {
                                out.extend_from_slice(&root.to_le_bytes());
                            }
                        }
                    }
                }
            }
            at.common_version_count = count(&mut out, self.common.len());
            for (v, n) in &self.common {
                out.extend_from_slice(&v.to_le_bytes());
                out.push(*n);
            }
            at.common_indices_count = count(&mut out, self.common_indices.len());
            for i in &self.common_indices {
                out.extend_from_slice(&i.to_le_bytes());
            }
            at.node_count = count(&mut out, self.nodes.len());
            for n in &self.nodes {
                out.extend_from_slice(&n.type_name.to_le_bytes());
                out.extend_from_slice(&n.name.to_le_bytes());
                out.extend_from_slice(&n.byte_size.to_le_bytes());
                out.extend_from_slice(&n.version.to_le_bytes());
                out.push(n.type_flags);
                out.extend_from_slice(&n.meta_flag.to_le_bytes());
                out.extend_from_slice(&u16::try_from(n.sub_nodes.len()).unwrap().to_le_bytes());
                for s in &n.sub_nodes {
                    out.extend_from_slice(&s.to_le_bytes());
                }
            }
            at.string_count = count(&mut out, self.strings.len());
            for s in &self.strings {
                varint(&mut out, s.len() as u64);
                out.extend_from_slice(s);
            }
            (out, at)
        }

        pub fn read(&self) -> Result<TpkTypeTreeBlob> {
            TpkTypeTreeBlob::from_reader(&mut self.write().0.as_slice())
        }
    }

    /// A TPK file around `payload`, as stored.
    pub(super) fn file(
        compression: u8,
        payload: &[u8],
        compressed: i32,
        uncompressed: i32,
    ) -> Vec<u8> {
        let mut out = b"TPK*".to_vec();
        out.extend_from_slice(&[1, compression, 0, 0]);
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.extend_from_slice(&compressed.to_le_bytes());
        out.extend_from_slice(&uncompressed.to_le_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// `Base base` with one child, `int x`, as class 1 at 2.0.0.
    pub(super) fn one_class() -> Blob {
        let mut blob = Blob::with_strings(4);
        blob.nodes = vec![Node::new(0, 1, vec![1]), Node::new(2, 3, vec![])];
        blob.classes = vec![(1, vec![(2 << 48, Some(Class::both(0)))])];
        blob
    }

    fn v(major: u16) -> UnityVersion {
        UnityVersion::new(major, 0, 0)
    }

    /// `blob` with the `i32` at `at` set to `n`.
    fn with_count(blob: &Blob, at: impl Fn(&Offsets) -> usize, n: i32) -> Vec<u8> {
        let (mut bytes, offsets) = blob.write();
        let at = at(&offsets);
        bytes[at..at + 4].copy_from_slice(&n.to_le_bytes());
        bytes
    }

    fn parse(bytes: &[u8]) -> Result<TpkTypeTreeBlob> {
        TpkTypeTreeBlob::from_reader(&mut &bytes[..])
    }

    /// Runs `f` on its own thread and fails the test if it takes 10 s (a
    /// walk that never ends) or panics.
    fn within_10s<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("no answer in 10 s (a hang), or a panic")
    }

    /// `n` nodes in a chain, node `i` listing node `i + 1`, as class 1.
    pub(super) fn chain(n: u16) -> Blob {
        let mut blob = Blob::with_strings(2);
        blob.nodes = (0..n)
            .map(|i| Node::new(0, 1, if i + 1 < n { vec![i + 1] } else { vec![] }))
            .collect();
        blob.classes = vec![(1, vec![(2 << 48, Some(Class::both(0)))])];
        blob
    }

    fn depth(node: &TypeTreeNode) -> usize {
        let mut depth = 1;
        let mut node = node;
        while let Some(child) = node.children.first() {
            depth += 1;
            node = child;
        }
        depth
    }

    #[test]
    fn f3_negative_header_sizes_are_errors_and_huge_ones_fail_at_the_end() {
        let payload = one_class().write().0;
        let len = i32::try_from(payload.len()).unwrap();
        for compression in [0, 1, 2] {
            for (compressed, uncompressed) in [(-1, len), (i32::MIN, len), (i32::MAX, len)] {
                let bytes = file(compression, &payload, compressed, uncompressed);
                assert!(TpkFile::from_reader(&mut bytes.as_slice()).is_err());
            }
        }
        // A negative uncompressed size: refused by each codec that needs it.
        for compression in [1, 2] {
            let bytes = file(compression, &payload, len, -1);
            let tpk = TpkFile::from_reader(&mut bytes.as_slice()).unwrap();
            assert!(tpk.as_type_tree().is_err(), "codec {compression}");
        }
        // A huge LZ4 size over a valid block cannot be met by it: refused
        // before the output is allocated at that size (upstream allocated
        // it and read the block).
        let block = lz4_flex::block::compress(&payload);
        let block_len = i32::try_from(block.len()).unwrap();
        let bytes = file(1, &block, block_len, len);
        let tpk = TpkFile::from_reader(&mut bytes.as_slice()).unwrap();
        assert_eq!(
            tpk.as_type_tree().unwrap().unwrap().class_information.len(),
            1
        );
        let bytes = file(1, &block, block_len, i32::MAX);
        let tpk = TpkFile::from_reader(&mut bytes.as_slice()).unwrap();
        assert!(tpk.as_type_tree().is_err());
        // Stored, the uncompressed size is not used.
        let bytes = file(0, &payload, len, -1);
        let tpk = TpkFile::from_reader(&mut bytes.as_slice()).unwrap();
        assert_eq!(
            tpk.as_type_tree().unwrap().unwrap().class_information.len(),
            1
        );
    }

    /// Each count is read as UnityPy's `Tpk.py` reads it: the version list's
    /// and the common-string indices' through a struct format (a negative
    /// count raises), the others through `range()` (a negative count reads
    /// nothing). None sizes an allocation, so a huge one fails when the
    /// bytes run out.
    #[test]
    fn f3_negative_and_huge_counts_read_as_unitypy_reads_them_and_never_panic() {
        let mut blob = one_class();
        blob.common = vec![(2 << 48, 1)];
        blob.common_indices = vec![0];
        type At = fn(&Offsets) -> usize;
        let at: [(&str, At); 7] = [
            ("versions", |o| o.version_count),
            ("classes", |o| o.class_count),
            ("entries", |o| o.entry_count),
            ("common versions", |o| o.common_version_count),
            ("common indices", |o| o.common_indices_count),
            ("nodes", |o| o.node_count),
            ("strings", |o| o.string_count),
        ];
        for (what, at) in at {
            for n in [i32::MAX, 1 << 30] {
                assert!(parse(&with_count(&blob, at, n)).is_err(), "{what} {n}");
            }
            for n in [-1, i32::MIN] {
                let got = parse(&with_count(&blob, at, n));
                match what {
                    "versions" | "common indices" => assert!(got.is_err(), "{what} {n}"),
                    // A negative count reads no item, and the item's bytes
                    // are read as what follows; whatever that gives, it is
                    // no panic.
                    _ => drop(got),
                }
            }
        }
        // Where nothing follows the count, a negative one is an empty table.
        let mut empty = Blob::with_strings(0);
        empty.versions = vec![];
        for at in [
            (|o: &Offsets| o.class_count) as At,
            |o| o.common_version_count,
            |o| o.node_count,
            |o| o.string_count,
        ] {
            let tpk = parse(&with_count(&empty, at, -1)).unwrap();
            assert!(tpk.class_information.is_empty() && tpk.node_buffer.is_empty());
        }
        // The two counts UnityPy reads as a struct format raise.
        for at in [(|o: &Offsets| o.version_count) as At, |o| {
            o.common_indices_count
        }] {
            assert!(parse(&with_count(&empty, at, -1)).is_err());
        }
        let mut no_entries = one_class();
        no_entries.classes = vec![(1, vec![])];
        let tpk = parse(&with_count(&no_entries, |o| o.entry_count, -1)).unwrap();
        assert!(tpk.class_information[&ClassId(1)].is_empty());
    }

    #[test]
    fn f3_every_flag_byte_reads_without_a_panic() {
        for flags in 0..=u8::MAX {
            let mut blob = one_class();
            blob.classes = vec![(
                1,
                vec![(
                    2 << 48,
                    Some(Class {
                        flags,
                        editor_root: Some(0),
                        release_root: Some(0),
                    }),
                )],
            )];
            let (mut bytes, _) = blob.write();
            // Whatever roots the flags name, both are written: a reader
            // that reads fewer reads the rest as what follows.
            let tpk = parse(&bytes);
            if let Ok(tpk) = tpk {
                let _ = tpk.try_get_typetree_node(ClassId(1), &v(2));
            }
            bytes.truncate(bytes.len() / 2);
            assert!(parse(&bytes).is_err());
        }
    }

    #[test]
    fn f3_a_node_or_string_id_past_its_table_is_an_error() {
        for (sub, type_name) in [(9, 0), (1, 9)] {
            let mut blob = one_class();
            blob.nodes[0].sub_nodes = vec![sub];
            blob.nodes[1].type_name = type_name;
            let tpk = blob.read().unwrap();
            assert!(tpk.try_get_typetree_node(ClassId(1), &v(2)).is_err());
            assert!(tpk.get_typetree_node(ClassId(1), &v(2)).is_none());
        }
        // The root itself past the buffer.
        let mut blob = one_class();
        blob.classes = vec![(1, vec![(2 << 48, Some(Class::both(5)))])];
        assert!(
            blob.read()
                .unwrap()
                .try_get_typetree_node(ClassId(1), &v(2))
                .is_err()
        );
    }

    #[test]
    fn f3_a_node_that_lists_itself_or_its_grandparent_is_an_error_not_a_hang() {
        // Node 0 lists itself; node 2 lists node 0, its grandparent.
        for nodes in [
            vec![Node::new(0, 1, vec![0])],
            vec![
                Node::new(0, 1, vec![1]),
                Node::new(0, 1, vec![2]),
                Node::new(0, 1, vec![0]),
            ],
        ] {
            let mut blob = Blob::with_strings(2);
            blob.nodes = nodes;
            blob.classes = vec![(1, vec![(2 << 48, Some(Class::both(0)))])];
            let tpk = blob.read().unwrap();
            let got = within_10s(move || tpk.try_get_typetree_node(ClassId(1), &v(2)).is_err());
            assert!(got);
        }
    }

    #[test]
    fn f3_a_300_deep_chain_walks_past_the_255_levels_a_u8_held() {
        let tpk = chain(300).read().unwrap();
        let tree = within_10s(move || tpk.try_get_typetree_node(ClassId(1), &v(2)).unwrap());
        assert_eq!(depth(&tree.unwrap()), 300);
    }

    #[test]
    fn f3_a_string_longer_than_the_data_is_an_error() {
        // The varint's largest length, 2^35 - 1, over a 3-byte tail.
        let mut blob = Blob::with_strings(0);
        blob.versions = vec![];
        let (mut bytes, at) = blob.write();
        bytes.truncate(at.string_count);
        bytes.extend_from_slice(&1_i32.to_le_bytes());
        varint(&mut bytes, (1 << 35) - 1);
        bytes.extend_from_slice(b"abc");
        assert!(parse(&bytes).is_err());
        // One byte short, and exact.
        for (tail, ok) in [(&b"ab"[..], false), (&b"abc"[..], true)] {
            let mut bytes = bytes[..bytes.len() - 3 - 5].to_vec();
            varint(&mut bytes, 3);
            bytes.extend_from_slice(tail);
            assert_eq!(parse(&bytes).is_ok(), ok);
        }
    }

    #[test]
    fn f3_the_common_string_count_scans_forward_as_unitypy_does() {
        let common = TpkCommonString {
            version_information: vec![(v(5), 3), (v(3), 1), (v(7), 9)],
            string_buffer_indices: vec![],
        };
        // UnityPy stops at the first version above the target (5 > 4).
        assert_eq!(common.get_count(&v(4)), None);
        assert_eq!(common.get_count(&v(6)), Some(1));
    }

    #[test]
    fn f5_an_lzma_tpk_reads_as_its_payload_not_behind_zeros() {
        let payload = one_class().write().0;
        let mut compressed = Vec::new();
        lzma_rs::lzma_compress_with_options(
            &mut payload.as_slice(),
            &mut compressed,
            &lzma_rs::compress::Options {
                unpacked_size: lzma_rs::compress::UnpackedSize::SkipWritingToHeader,
            },
        )
        .unwrap();
        let bytes = file(
            2,
            &compressed,
            i32::try_from(compressed.len()).unwrap(),
            i32::try_from(payload.len()).unwrap(),
        );
        let tpk = TpkFile::from_reader(&mut bytes.as_slice()).unwrap();
        assert_eq!(&*tpk.decompress().unwrap(), &payload[..]);
        let blob = tpk.as_type_tree().unwrap().unwrap();
        assert_eq!(blob.versions.len(), 1);
        assert_eq!(blob.class_information.len(), 1);
        assert!(blob.get_typetree_node(ClassId(1), &v(2)).is_some());
        // A size the stream does not reach is an error, not a short blob.
        let short = file(
            2,
            &compressed,
            i32::try_from(compressed.len()).unwrap(),
            i32::try_from(payload.len() + 1).unwrap(),
        );
        let tpk = TpkFile::from_reader(&mut short.as_slice()).unwrap();
        assert!(tpk.as_type_tree().is_err());
    }

    /// Classes 10, 20, 30 and 40 have the root flags 0, 64, 128 and 192;
    /// each is followed by a class with both roots (11, 21, 31, 41), which
    /// reads wrong when the one before it is misread.
    #[test]
    fn f6_each_root_is_read_under_its_own_flag() {
        let mut blob = one_class();
        blob.classes.clear();
        for (id, flags) in [(10, 0), (20, 64), (30, 128), (40, 192)] {
            let class = Class {
                flags,
                editor_root: (flags & 64 != 0).then_some(1),
                release_root: (flags & 128 != 0).then_some(0),
            };
            blob.classes.push((id, vec![(2 << 48, Some(class))]));
            blob.classes
                .push((id + 1, vec![(2 << 48, Some(Class::both(0)))]));
        }
        let tpk = blob.read().unwrap();
        for (id, release) in [(10, false), (20, false), (30, true), (40, true)] {
            let class = tpk.class_information[&ClassId(id)][0].1.as_ref().unwrap();
            assert_eq!(class.release_root_node.is_some(), release, "{id}");
            assert_eq!(
                class.editor_root_node.is_some(),
                id == 20 || id == 40,
                "{id}"
            );
            let tree = tpk.try_get_typetree_node(ClassId(id), &v(2)).unwrap();
            assert_eq!(tree.is_some(), release, "{id}");
            let next = tpk.try_get_typetree_node(ClassId(id + 1), &v(2)).unwrap();
            assert_eq!(next.unwrap().children.len(), 1, "{}", id + 1);
        }
        let release_only = tpk.get_typetree_node(ClassId(30), &v(2)).unwrap();
        assert_eq!(release_only.m_Type, "s0");
        assert_eq!(release_only.children[0].m_Name, "s3");
    }

    #[test]
    fn f2_an_unknown_class_has_no_tree() {
        let tpk = one_class().read().unwrap();
        assert!(tpk.get_typetree_node(ClassId(1), &v(2)).is_some());
        assert!(tpk.get_typetree_node(ClassId(2), &v(2)).is_none());
        assert!(tpk.get_typetree_node(ClassId(-1), &v(2)).is_none());
    }
}
