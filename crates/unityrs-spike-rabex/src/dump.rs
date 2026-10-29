//! Canonical dump records (docs/CANONICAL_DUMP.md section 4) from rabex's
//! containers, SerializedFiles and typetrees.
//!
//! Where rabex keeps no record of something the format needs (the bundle's
//! data flags and blocks-info row, raw version strings, absent-versus-default
//! fields), the gap is filled here and marked `LAYER` in a comment; where it
//! reads differently from UnityPy, the dump shows what rabex read.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

use byteorder::{BigEndian, LittleEndian};
use rabex::UnityVersion;
use rabex::files::bundlefile::{BundleFile, ExtractionConfig};
use rabex::files::serializedfile::{Endianness, ObjectInfo, SerializedType};
use rabex::files::{SerializedFile, unityfile::FileEntry};
use rabex::objects::ClassId;
use rabex::serde_typetree;
use rabex::typetree::TypeTreeNode;
use serde::de::DeserializeSeed;

use crate::detect::{FileType, check_file_type, parsed_as};
use crate::encode::Encoder;
use crate::json::{J, hex, sha256_hex};
use crate::tpk::Tpk;
use crate::value::{Seed, Ser, Val};

#[derive(Clone, Copy)]
pub struct Levels {
    pub l1: bool,
    pub l2: bool,
    pub l3: bool,
}

#[derive(Default)]
pub struct Stats {
    pub files: u64,
    pub objects: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub load_errors: u64,
    pub counts: BTreeMap<String, u64>,
    pub t_load: f64,
    pub t_parse: f64,
    pub t_encode: f64,
    pub t_resave: f64,
}

impl Stats {
    pub fn bump(&mut self, key: &str) {
        *self.counts.entry(key.to_owned()).or_default() += 1;
    }

    pub fn merge(&mut self, o: &Stats) {
        self.files += o.files;
        self.objects += o.objects;
        self.bytes_in += o.bytes_in;
        self.bytes_out += o.bytes_out;
        self.load_errors += o.load_errors;
        for (k, v) in &o.counts {
            *self.counts.entry(k.clone()).or_default() += v;
        }
        self.t_load += o.t_load;
        self.t_parse += o.t_parse;
        self.t_encode += o.t_encode;
        self.t_resave += o.t_resave;
    }
}

pub struct Ctx<'a> {
    pub tpk: &'a Tpk,
    pub levels: Levels,
    /// Work around rabex 0.2.1's UnityFS header read (see [`shim_fs_header`]).
    pub shim: bool,
}

/// A panic's message, for notes. rabex panics (`todo!`, `unimplemented!`,
/// unchecked indexing) on inputs it does not handle; each one is a finding.
fn panic_note(e: &(dyn std::any::Any + Send)) -> String {
    let msg = e
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    let first = msg.lines().next().unwrap_or_default();
    format!("panic: {first}")
}

fn guard<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|e| Err(panic_note(&*e)))
}

fn note(s: &str) -> J {
    const MAX: usize = 300;
    if s.len() <= MAX {
        return J::s(s);
    }
    let mut cut = MAX - 3;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    J::S(format!("{}...", &s[..cut]))
}

// ------------------------------------------------------------------ parsing

/// A parsed file, as UnityPy's `parse_file` would build it.
enum Node {
    Bundle(Box<Bundle>),
    Serialized(Box<Serialized>),
    Raw,
}

struct Bundle {
    file: BundleFile,
    head: HeadExtra,
    /// UnityPy's `files` dict: directory order, first position, last entry.
    children: Vec<Child>,
}

struct Child {
    name: String,
    start: usize,
    end: usize,
    ty: FileType,
    parsed_as: &'static str,
    node: Node,
}

struct Serialized {
    sf: SerializedFile,
    parent_engine: Option<UnityVersion>,
    container: Vec<J>,
}

/// LAYER: what rabex reads from a UnityFS header and then drops.
struct HeadExtra {
    flags: Option<u32>,
    blocks_info: Option<[u32; 3]>,
}

/// The end of a UnityFS header (signature, format, two version strings, then
/// size and the three blocks-info u32s), and its format.
fn fs_header_end(data: &[u8]) -> Option<(usize, u32)> {
    let mut pos = 0;
    let skip_cstr = |pos: &mut usize| -> Option<()> {
        *pos += data.get(*pos..)?.iter().position(|&b| b == 0)? + 1;
        Some(())
    };
    skip_cstr(&mut pos)?;
    let format = u32::from_be_bytes(data.get(pos..pos + 4)?.try_into().ok()?);
    pos += 4;
    skip_cstr(&mut pos)?;
    skip_cstr(&mut pos)?;
    Some((pos + 20, format))
}

/// rabex 0.2.1 reads one byte after a UnityFS header that UnityPy reads only
/// for UnityWeb/UnityRaw format 6 (`read_unityfs_info`, `if let
/// BundleSignature::UnityFS`). At format 6 that shifts every later read by
/// one; at format 7+ the 16-byte alignment absorbs it unless the header ends
/// on a 16-byte boundary. Padding the header where rabex reads gives the
/// result of the one-line upstream fix, so the layers above can be measured.
pub fn shim_fs_header(data: &[u8]) -> Option<Vec<u8>> {
    if !data.starts_with(b"UnityFS\0") {
        return None;
    }
    let (end, format) = fs_header_end(data)?;
    let pad = match format {
        6 => 1,
        f if f >= 7 && end % 16 == 0 => 16,
        _ => return None,
    };
    let mut out = Vec::with_capacity(data.len() + pad);
    out.extend_from_slice(data.get(..end)?);
    out.resize(end + pad, 0);
    out.extend_from_slice(&data[end..]);
    Some(out)
}

/// A bundle's bytes as rabex should read them: shimmed only when asked
/// (`--shim-fs6`), so every subcommand measures rabex as published by default.
pub fn input_bytes(data: &[u8], shim: bool) -> Cow<'_, [u8]> {
    match shim.then(|| shim_fs_header(data)).flatten() {
        Some(v) => Cow::Owned(v),
        None => Cow::Borrowed(data),
    }
}

fn read_head_extra(data: &[u8]) -> HeadExtra {
    let none = HeadExtra {
        flags: None,
        blocks_info: None,
    };
    let mut pos = 0;
    let cstr = |pos: &mut usize| -> Option<Vec<u8>> {
        let end = data.get(*pos..)?.iter().position(|&b| b == 0)?;
        let s = data[*pos..*pos + end].to_vec();
        *pos += end + 1;
        Some(s)
    };
    let Some(sig) = cstr(&mut pos) else {
        return none;
    };
    let Some(format) = data.get(pos..pos + 4) else {
        return none;
    };
    let format = u32::from_be_bytes(format.try_into().unwrap());
    pos += 4;
    let (Some(_), Some(_)) = (cstr(&mut pos), cstr(&mut pos)) else {
        return none;
    };
    if !(sig == b"UnityFS" || format == 6) {
        return none;
    }
    let Some(h) = data.get(pos + 8..pos + 20) else {
        return none;
    };
    let u = |o: usize| u32::from_be_bytes(h[o..o + 4].try_into().unwrap());
    let (compressed, uncompressed, flags) = (u(0), u(4), u(8));
    HeadExtra {
        flags: Some(flags),
        blocks_info: Some([uncompressed, compressed, flags]),
    }
}

fn entry_range(e: &FileEntry, len: usize) -> Result<(usize, usize), String> {
    let start = usize::try_from(e.offset).map_err(|_| "negative entry offset".to_owned())?;
    let size = usize::try_from(e.size).map_err(|_| "negative entry size".to_owned())?;
    let end = start.checked_add(size).filter(|&end| end <= len);
    end.map(|end| (start, end))
        .ok_or_else(|| format!("entry {} runs past the bundle data", e.path))
}

fn parse(
    data: &[u8],
    name: &str,
    ty: FileType,
    parent_engine: Option<&UnityVersion>,
    ctx: &Ctx,
) -> Result<Node, String> {
    match parsed_as(ty, name) {
        "BundleFile" => {
            let config = ExtractionConfig::default();
            let input = input_bytes(data, ctx.shim);
            let file = BundleFile::from_reader(&mut Cursor::new(&*input), &config)
                .map_err(|e| format!("rabex bundle: {e}"))?;
            let head = read_head_extra(data);
            let mut children: Vec<Child> = Vec::new();
            for e in &file.m_DirectoryInfo {
                let (start, end) = entry_range(e, file.m_BlockData.len())?;
                let bytes = &file.m_BlockData[start..end];
                let cty = check_file_type(bytes);
                let engine = file.m_Header.unity_revision.as_ref();
                let child = Child {
                    name: e.path.clone(),
                    start,
                    end,
                    ty: cty,
                    parsed_as: parsed_as(cty, &e.path),
                    node: parse(bytes, &e.path, cty, engine, ctx)?,
                };
                match children.iter_mut().find(|c| c.name == child.name) {
                    Some(slot) => *slot = child,
                    None => children.push(child),
                }
            }
            Ok(Node::Bundle(Box::new(Bundle {
                file,
                head,
                children,
            })))
        }
        "SerializedFile" => {
            let sf = SerializedFile::from_reader(&mut Cursor::new(data))
                .map_err(|e| format!("rabex serialized: {e}"))?;
            let mut s = Serialized {
                sf,
                parent_engine: parent_engine.cloned(),
                container: Vec::new(),
            };
            // UnityPy parses the AssetBundle while loading; a failure fails the file.
            s.container = container_rows(&s, data, ctx)?;
            Ok(Node::Serialized(Box::new(s)))
        }
        "WebFile" => Err("rabex has no WebFile reader".to_owned()),
        _ => Ok(Node::Raw),
    }
}

/// UnityPy's `objects` dict: path_id order of first appearance, last entry wins.
fn object_dict(sf: &SerializedFile) -> Vec<&ObjectInfo> {
    let mut index: HashMap<i64, usize> = HashMap::new();
    let mut out: Vec<&ObjectInfo> = Vec::new();
    for o in sf.objects() {
        if let Some(&i) = index.get(&o.m_PathID) {
            out[i] = o;
        } else {
            index.insert(o.m_PathID, out.len());
            out.push(o);
        }
    }
    out
}

/// The value of a dict's `name` entry (the last child of that name, as in a PyDict).
fn field<'v, 't>(
    v: &'v Val,
    node: &'t TypeTreeNode,
    name: &str,
) -> Option<(&'v Val, &'t TypeTreeNode)> {
    let Val::Dict(vals) = v else { return None };
    let i = node.children.iter().rposition(|c| c.m_Name == name)?;
    Some((vals.get(i)?, &node.children[i]))
}

/// LAYER: the AssetBundle's `m_Container`, read through rabex as a typetree value.
fn container_rows(s: &Serialized, data: &[u8], ctx: &Ctx) -> Result<Vec<J>, String> {
    let Some(info) = object_dict(&s.sf)
        .into_iter()
        .find(|o| o.m_ClassID == ClassId::AssetBundle)
    else {
        return Ok(Vec::new());
    };
    let tt =
        s.sf.get_typetree_for(info, ctx.tpk)
            .map_err(|e| format!("AssetBundle typetree: {e}"))?;
    let raw = object_bytes(data, info)?;
    let (val, _) = read_value(raw, &tt, s.sf.m_Header.m_Endianess)
        .map_err(|e| format!("AssetBundle: {}", e.1))?;
    let Some((Val::List(items), cnode)) = field(&val, &tt, "m_Container") else {
        return Ok(Vec::new());
    };
    let pair = crate::value::element(cnode).ok_or("m_Container has no Array")?;
    let int = |f: Option<(&Val, &TypeTreeNode)>| match f {
        Some((Val::I(i), _)) => J::I(*i),
        _ => J::Null,
    };
    let mut rows = Vec::new();
    for item in items {
        let Val::Tuple(kv) = item else {
            return Err("m_Container entry is not a pair".to_owned());
        };
        let path = match &kv[0] {
            Val::Str(s) => J::s(s),
            Val::BadStr(b) => J::O(vec![("t", J::s("str")), ("hex", J::S(hex(b)))]),
            _ => J::Null,
        };
        let info_node = &pair.children[1];
        let asset = field(&kv[1], info_node, "asset");
        let pptr = |name| asset.and_then(|(a, an)| field(a, an, name));
        rows.push(J::A(vec![
            path,
            int(field(&kv[1], info_node, "preloadIndex")),
            int(field(&kv[1], info_node, "preloadSize")),
            int(pptr("m_FileID")),
            int(pptr("m_PathID")),
        ]));
    }
    Ok(rows)
}

fn object_bytes<'d>(data: &'d [u8], info: &ObjectInfo) -> Result<&'d [u8], String> {
    let start = usize::try_from(info.m_Offset).map_err(|_| "negative object offset".to_owned())?;
    let end = start + info.m_Size as usize;
    data.get(start..end.min(data.len()))
        .ok_or_else(|| "object starts past the file".to_owned())
}

/// Reads one value through rabex, bounded by the object's own bytes as
/// UnityPyBoost is. Returns the value and the bytes consumed, or the error
/// kind and message.
pub fn read_value(
    raw: &[u8],
    node: &TypeTreeNode,
    endian: Endianness,
) -> Result<(Val, usize), (&'static str, String)> {
    let mut cur = Cursor::new(raw);
    let result = catch_unwind(AssertUnwindSafe(|| match endian {
        Endianness::Little => {
            let mut de =
                serde_typetree::Deserializer::<_, LittleEndian>::from_reader(&mut cur, node);
            Seed(node).deserialize(&mut de)
        }
        Endianness::Big => {
            let mut de = serde_typetree::Deserializer::<_, BigEndian>::from_reader(&mut cur, node);
            Seed(node).deserialize(&mut de)
        }
    }));
    match result {
        Ok(Ok(v)) => Ok((v, usize::try_from(cur.position()).unwrap_or(usize::MAX))),
        Ok(Err(e)) => {
            let msg = e.to_string();
            Err((error_kind(&msg), msg))
        }
        Err(p) => Err(("other", panic_note(&*p))),
    }
}

/// rabex's serde errors are opaque (`ErrorImpl` is private), so the spec's
/// error kinds come from the message text.
fn error_kind(msg: &str) -> &'static str {
    if msg.contains("failed to fill whole buffer") || msg.contains("unexpected end of file") {
        "out_of_bounds"
    } else {
        "other"
    }
}

// ------------------------------------------------------------------ records

fn pack_version(v: &UnityVersion) -> J {
    // UnityPy's plain OR: a build number above 255 spills into the type byte.
    // rabex keeps the build number as a u8, so it never does.
    let packed = (u64::from(v.major) << 48)
        | (u64::from(v.minor) << 32)
        | (u64::from(v.build) << 16)
        | (u64::from(v.typ as u8) << 8)
        | u64::from(v.build_number);
    J::U(packed)
}

fn version_string(v: Option<&UnityVersion>) -> J {
    J::S(v.map_or_else(|| "0.0.0".to_owned(), ToString::to_string))
}

/// UnityPy's ArchiveFlagsOld rule by engine version (rabex applies the same
/// rule privately in `use_new_archive_flags`).
fn flags_class(v: Option<&UnityVersion>) -> J {
    let Some(v) = v else { return J::Null };
    let t = v.version_tuple();
    let old = t < (2020, 0, 0)
        || (t.0 == 2020 && t < (2020, 3, 34))
        || (t.0 == 2021 && t < (2021, 3, 2))
        || (t.0 == 2022 && t < (2022, 1, 1));
    J::s(if old {
        "ArchiveFlagsOld"
    } else {
        "ArchiveFlags"
    })
}

fn bundle_l1(b: &Bundle) -> J {
    let h = &b.file.m_Header;
    let fs = h.signature.to_string() == "UnityFS" || h.version == 6;
    let engine = h.unity_revision.as_ref();
    J::O(vec![
        ("kind", J::s("bundle")),
        ("signature", J::S(h.signature.to_string())),
        ("format", J::U(u64::from(h.version))),
        ("version_player", J::s(&h.unity_version)),
        // rabex keeps the engine string only as a parsed UnityVersion.
        ("version_engine", version_string(engine)),
        ("engine_version_packed", J::opt(engine, pack_version)),
        ("flags", J::opt(b.head.flags, |f| J::U(u64::from(f)))),
        (
            "flags_class",
            if fs { flags_class(engine) } else { J::Null },
        ),
        // rabex aligns at format >= 7 only (no 2019.4.15+ format-6 rule).
        ("block_alignment", J::Bool(h.version >= 7)),
        ("encrypted", J::Bool(false)),
        ("hash", J::Null),
        ("crc", J::Null),
        (
            "blocks_info",
            J::opt(b.head.blocks_info, |r| {
                J::A(r.iter().map(|&x| J::U(u64::from(x))).collect())
            }),
        ),
        (
            "blocks",
            if fs {
                J::A(
                    b.file
                        .m_BlocksInfo
                        .iter()
                        .map(|s| {
                            J::A(vec![
                                J::U(u64::from(s.uncompressed_size)),
                                J::U(u64::from(s.compressed_size)),
                                J::U(u64::from(s.flags)),
                            ])
                        })
                        .collect(),
                )
            } else {
                J::Null
            },
        ),
        (
            "directory",
            J::A(
                b.file
                    .m_DirectoryInfo
                    .iter()
                    .map(|e| {
                        J::A(vec![
                            J::I(e.offset),
                            J::I(e.size),
                            if fs {
                                J::U(u64::from(e.flags))
                            } else {
                                J::Null
                            },
                            J::s(&e.path),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn node_rows(root: &TypeTreeNode) -> J {
    fn walk(n: &TypeTreeNode, level: u64, rows: &mut Vec<J>) {
        let index = rows.len() as u64;
        rows.push(J::A(vec![
            J::U(level),
            J::s(&n.m_Type),
            J::s(&n.m_Name),
            J::I(i64::from(n.m_ByteSize)),
            J::I(i64::from(n.m_Version)),
            J::I(i64::from(n.m_TypeFlags)),
            J::opt(n.m_VariableCount, |v| J::I(i64::from(v))),
            // rabex drops the stored m_Index; pre-order position is what Unity writes.
            J::U(index),
            J::opt(n.m_MetaFlag, |v| J::I(i64::from(v))),
            J::opt(n.m_RefTypeHash, J::U),
        ]));
        for c in &n.children {
            walk(c, level + 1, rows);
        }
    }
    let mut rows = Vec::new();
    walk(root, 0, &mut rows);
    J::A(rows)
}

fn type_record(t: &SerializedType, version: u32, is_ref: bool, enable_tree: bool) -> J {
    // LAYER: rabex fills absent fields with defaults; UnityPy leaves them None.
    let has_script_id = version >= 13
        && ((is_ref && t.m_ScriptTypeIndex >= 0)
            || (version < 16 && t.m_ClassID.0 < 0)
            || (version >= 16 && t.m_ClassID == ClassId::MonoBehaviour));
    let deps = enable_tree && version >= 21 && !is_ref;
    let names = enable_tree && version >= 21 && is_ref;
    J::O(vec![
        ("class_id", J::I(i64::from(t.m_ClassID.0))),
        (
            "is_stripped_type",
            if version >= 16 {
                J::Bool(t.m_IsStrippedType)
            } else {
                J::Null
            },
        ),
        ("script_type_index", J::I(i64::from(t.m_ScriptTypeIndex))),
        (
            "script_id",
            if has_script_id {
                J::S(hex(&t.m_ScriptID))
            } else {
                J::Null
            },
        ),
        (
            "old_type_hash",
            if version >= 13 {
                J::S(hex(&t.m_OldTypeHash))
            } else {
                J::Null
            },
        ),
        ("nodes", J::opt(t.m_Type.as_ref(), node_rows)),
        (
            "type_dependencies",
            if deps {
                J::A(
                    t.m_TypeDependencies
                        .iter()
                        .map(|&d| J::I(i64::from(d)))
                        .collect(),
                )
            } else {
                J::Null
            },
        ),
        (
            "class_name",
            if names {
                J::opt(t.m_ClassName.as_deref(), J::s)
            } else {
                J::Null
            },
        ),
        (
            "namespace",
            if names {
                J::opt(t.m_NameSpace.as_deref(), J::s)
            } else {
                J::Null
            },
        ),
        (
            "assembly",
            if names {
                J::opt(t.m_AsmName.as_deref(), J::s)
            } else {
                J::Null
            },
        ),
    ])
}

fn effective_version(s: &Serialized) -> Option<&UnityVersion> {
    s.sf.m_UnityVersion.as_ref().or(s.parent_engine.as_ref())
}

#[allow(clippy::too_many_lines)] // the record, member by member, in spec order
fn serialized_l1(s: &Serialized) -> J {
    let sf = &s.sf;
    let h = &sf.m_Header;
    let v = h.m_Version;
    let tree = sf.m_EnableTypeTree;
    J::O(vec![
        ("kind", J::s("serialized")),
        (
            "header",
            J::O(vec![
                ("metadata_size", J::U(u64::from(h.m_MetadataSize))),
                ("file_size", J::I(h.m_FileSize)),
                ("version", J::U(u64::from(v))),
                ("data_offset", J::I(h.m_DataOffset)),
                (
                    "endian",
                    J::s(if h.m_Endianess == Endianness::Big {
                        ">"
                    } else {
                        "<"
                    }),
                ),
                (
                    "reserved",
                    if v >= 9 {
                        J::S(hex(&h.m_Reserved))
                    } else {
                        J::Null
                    },
                ),
                ("unknown", J::I(h.unknown)),
            ]),
        ),
        // rabex keeps only the parsed version ("0.0.0" becomes None).
        ("unity_version", version_string(sf.m_UnityVersion.as_ref())),
        (
            "version_packed",
            if v >= 7 {
                J::opt(effective_version(s), pack_version)
            } else {
                J::Null
            },
        ),
        (
            "target_platform",
            J::opt(sf.m_TargetPlatform, |p| J::I(i64::from(p))),
        ),
        ("enable_type_tree", J::Bool(tree)),
        // rabex reads bigIDEnabled and discards it (m_bigIDEnabled stays None).
        (
            "big_id_enabled",
            J::I(i64::from(sf.m_bigIDEnabled.unwrap_or(0))),
        ),
        (
            "types",
            J::A(
                sf.m_Types
                    .iter()
                    .map(|t| type_record(t, v, false, tree))
                    .collect(),
            ),
        ),
        (
            "object_order",
            J::A(object_dict(sf).iter().map(|o| J::I(o.m_PathID)).collect()),
        ),
        (
            "script_types",
            J::A(
                sf.m_ScriptTypes
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|s| {
                        J::A(vec![
                            J::I(i64::from(s.m_LocalSerializedFileIndex.value())),
                            J::I(s.m_LocalIdentifierInFile),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "externals",
            J::A(
                sf.m_Externals
                    .iter()
                    .map(|e| {
                        J::A(vec![
                            J::opt(e.tempEmpty.as_deref(), J::s),
                            J::opt(e.guid.as_ref(), |g| J::S(hex(&g.0))),
                            J::opt(e.typeId, |t| J::I(i64::from(t))),
                            J::s(&e.pathName),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "ref_types",
            J::opt(sf.m_RefTypes.as_ref(), |r| {
                J::A(r.iter().map(|t| type_record(t, v, true, tree)).collect())
            }),
        ),
        (
            "user_information",
            J::opt(sf.m_UserInformation.as_deref(), J::s),
        ),
        ("container", J::A(s.container.clone())),
    ])
}

fn file_record(path: &[String], l0: J, l1: J, error: J) -> J {
    J::O(vec![
        ("rec", J::s("file")),
        ("file", J::A(path.iter().map(|p| J::s(p)).collect())),
        ("l0", l0),
        ("l1", l1),
        ("error", error),
    ])
}

fn l0(ty: FileType, parsed: &str, data: &[u8]) -> J {
    J::O(vec![
        ("type", J::s(ty.name())),
        ("parsed_as", J::s(parsed)),
        ("size", J::U(data.len() as u64)),
        ("sha256", J::S(sha256_hex(data))),
    ])
}

struct Emitter<'c, 'a> {
    ctx: &'c Ctx<'a>,
    out: Vec<String>,
    stats: Stats,
}

impl Emitter<'_, '_> {
    fn emit(&mut self, node: &Node, data: &[u8], path: &[String], l0: J) {
        let levels = self.ctx.levels;
        let l1 = match node {
            Node::Bundle(b) if levels.l1 => bundle_l1(b),
            Node::Serialized(s) if levels.l1 => serialized_l1(s),
            _ => J::Null,
        };
        self.out.push(file_record(path, l0, l1, J::Null).line());
        self.stats.files += 1;
        match node {
            Node::Bundle(b) => {
                for c in &b.children {
                    let bytes = &b.file.m_BlockData[c.start..c.end];
                    let mut p = path.to_vec();
                    p.push(c.name.clone());
                    self.emit(&c.node, bytes, &p, l0_of(c, bytes));
                }
            }
            Node::Serialized(s) => self.emit_objects(s, data, path),
            Node::Raw => {}
        }
    }

    fn emit_objects(&mut self, s: &Serialized, data: &[u8], path: &[String]) {
        let mut objects = object_dict(&s.sf);
        objects.sort_by_key(|o| o.m_PathID);
        let mut enc = Encoder::default();
        let path_j = || J::A(path.iter().map(|p| J::s(p)).collect());
        for info in objects {
            let rec = self.object_record(s, info, data, &mut enc);
            let mut members = vec![
                ("rec", J::s("object")),
                ("file", path_j()),
                ("path_id", J::I(info.m_PathID)),
            ];
            members.extend(rec);
            self.out.push(J::O(members).line());
            self.stats.objects += 1;
        }
    }

    #[allow(clippy::too_many_lines)] // L1, L2 and L3 of one object, in order
    fn object_record(
        &mut self,
        s: &Serialized,
        info: &ObjectInfo,
        data: &[u8],
        enc: &mut Encoder,
    ) -> Vec<(&'static str, J)> {
        let levels = self.ctx.levels;
        let sf = &s.sf;
        let l1 = if levels.l1 {
            J::O(vec![
                ("type_id", J::I(i64::from(info.m_TypeID))),
                ("class_id", J::I(i64::from(info.m_ClassID.0))),
                ("byte_start", J::I(info.m_Offset)),
                ("byte_size", J::U(u64::from(info.m_Size))),
                (
                    "is_destroyed",
                    J::opt(info.m_IsDestroyed, |v| J::U(u64::from(v))),
                ),
                (
                    "is_stripped",
                    J::opt(info.m_Stripped, |v| J::U(u64::from(v))),
                ),
            ])
        } else {
            J::Null
        };
        let raw = object_bytes(data, info).unwrap_or_default();
        let mut parsed: Option<(Val, Cow<TypeTreeNode>)> = None;
        let mut l2 = J::Null;
        if levels.l2 {
            let t0 = Instant::now();
            let tt = guard(|| {
                sf.get_typetree_for(info, self.ctx.tpk)
                    .map_err(|e| e.to_string())
            });
            let (root_type, source, mut error, mut value) = match tt {
                Err(e) => (J::Null, J::Null, Some(("no_typetree", None, e)), None),
                Ok(tt) => {
                    let embedded = sf.m_Header.m_Version >= 16
                        && sf
                            .m_Types
                            .get(usize::try_from(info.m_TypeID).unwrap_or(usize::MAX))
                            .is_some_and(|t| t.m_Type.is_some());
                    let root = J::s(&tt.m_Type);
                    let source = J::s(if embedded { "embedded" } else { "tpk" });
                    match read_value(raw, &tt, sf.m_Header.m_Endianess) {
                        Ok((v, n)) if n < raw.len() => {
                            let note = format!(
                                "Expected to read {} bytes, but only read {n} bytes",
                                raw.len()
                            );
                            (
                                root,
                                source,
                                Some(("check_read", Some(n), note)),
                                Some((v, tt)),
                            )
                        }
                        Ok((v, _)) => (root, source, None, Some((v, tt))),
                        Err((kind, msg)) => (root, source, Some((kind, None, msg)), None),
                    }
                }
            };
            let t1 = Instant::now();
            let mut encoded = J::Null;
            if let Some((v, tt)) = &value {
                let mut text = String::new();
                match guard(|| enc.encode(&mut text, v, tt)) {
                    Ok(()) => encoded = J::Raw(text),
                    Err(e) => {
                        error = Some(("encode", None, e));
                        self.stats.bump("encode_errors");
                    }
                }
            }
            if let Some((kind, _, _)) = &error {
                self.stats.bump(&format!("l2_{kind}"));
                if *kind != "check_read" {
                    value = None;
                    encoded = J::Null;
                }
            }
            self.stats.t_parse += (t1 - t0).as_secs_f64();
            self.stats.t_encode += t1.elapsed().as_secs_f64();
            l2 = J::O(vec![
                ("root_type", root_type),
                ("source", source),
                (
                    "error",
                    match &error {
                        None => J::Null,
                        Some((kind, read, msg)) => J::O(vec![
                            ("kind", J::s(kind)),
                            ("bytes_read", J::opt(*read, |n| J::U(n as u64))),
                            ("note", note(msg)),
                        ]),
                    },
                ),
                ("value", encoded),
            ]);
            if error.is_none() {
                parsed = value;
            }
        }
        let mut l3 = J::Null;
        if levels.l3 {
            let t0 = Instant::now();
            let mut m = vec![
                ("raw_size", J::U(raw.len() as u64)),
                ("raw_sha256", J::S(sha256_hex(raw))),
            ];
            let outcome = match &parsed {
                None => "skipped",
                Some((v, tt)) => {
                    let ser = Ser { v, node: tt };
                    match guard(|| {
                        serde_typetree::to_vec_endianed(&ser, tt, sf.m_Header.m_Endianess)
                            .map_err(|e| e.to_string())
                    }) {
                        Ok(out) if out == raw => "equal",
                        Ok(out) => {
                            let first = out
                                .iter()
                                .zip(raw)
                                .position(|(a, b)| a != b)
                                .unwrap_or(out.len().min(raw.len()));
                            m.push(("resave", J::s("differs")));
                            m.push(("resave_size", J::U(out.len() as u64)));
                            m.push(("resave_sha256", J::S(sha256_hex(&out))));
                            m.push(("first_diff", J::U(first as u64)));
                            "differs"
                        }
                        Err(e) => {
                            m.extend([
                                ("resave", J::s("error")),
                                ("resave_size", J::Null),
                                ("resave_sha256", J::Null),
                                ("first_diff", J::Null),
                                ("note", note(&e)),
                            ]);
                            "error"
                        }
                    }
                }
            };
            if matches!(outcome, "skipped" | "equal") {
                m.extend([
                    ("resave", J::s(outcome)),
                    ("resave_size", J::Null),
                    ("resave_sha256", J::Null),
                    ("first_diff", J::Null),
                ]);
            }
            self.stats.bump(&format!("l3_{outcome}"));
            self.stats.t_resave += t0.elapsed().as_secs_f64();
            l3 = J::O(m);
        }
        vec![("l1", l1), ("l2", l2), ("l3", l3)]
    }
}

fn l0_of(c: &Child, bytes: &[u8]) -> J {
    l0(c.ty, c.parsed_as, bytes)
}

/// One input to canonical lines (its records, without header or end).
pub fn dump_input(data: &[u8], rel: &str, name: &str, ctx: &Ctx) -> (String, Stats) {
    let t0 = Instant::now();
    let ty = check_file_type(data);
    let mut em = Emitter {
        ctx,
        out: Vec::new(),
        stats: Stats::default(),
    };
    let path = vec![rel.to_owned()];
    match ty {
        FileType::Zip | FileType::Error => {
            let err = if ty == FileType::Error {
                J::O(vec![("kind", J::s("detect"))])
            } else {
                J::Null
            };
            em.out
                .push(file_record(&path, l0(ty, parsed_as(ty, name), data), J::Null, err).line());
            em.stats.files += 1;
        }
        _ => match guard(|| parse(data, name, ty, None, ctx)) {
            Err(e) => {
                em.out.push(
                    file_record(
                        &path,
                        l0(ty, "error", data),
                        J::Null,
                        J::O(vec![("kind", J::s("parse")), ("note", note(&e))]),
                    )
                    .line(),
                );
                em.stats.files += 1;
                em.stats.load_errors += 1;
                em.stats.bump(&format!(
                    "load_error: {}",
                    e.split(':').take(2).collect::<Vec<_>>().join(":")
                ));
            }
            Ok(node) => {
                em.stats.t_load += t0.elapsed().as_secs_f64();
                em.emit(&node, data, &path, l0(ty, parsed_as(ty, name), data));
            }
        },
    }
    let text: String = em.out.concat();
    em.stats.bytes_in = data.len() as u64;
    em.stats.bytes_out = text.len() as u64;
    (text, em.stats)
}
