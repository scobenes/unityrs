// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/SerializedFile.py, UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! [`SerializedFile::parse`]: `SerializedFile.__init__`'s metadata read,
//! by the rules the `serialized` module docs list, and the `objects` dict's
//! order.

use std::sync::Arc;

use bytes::Bytes;

use super::{External, ObjectInfo, ScriptRef, SerializedFile, SerializedHeader, SerializedType};
use crate::error::Error;
use crate::files::{Parent, ParentKind};
use crate::limits::{Charge, Limits, Meter, MeteredVec, heap_cost};
use crate::once::ResultCell;
use crate::options::FileContext;
use crate::raw::RawStr;
use crate::stream::{Endian, Reader};
use crate::typetree::NodeList;
use crate::typetree::commonstring::CommonStrings;
use crate::typetree::parse::{is_blob_format, parse_blob, parse_old, string_to_null};
use crate::version::resolve_serialized;

impl SerializedFile {
    /// `SerializedFile.__init__`. Everything it keeps is charged to `meter`,
    /// the input's.
    pub fn parse(
        name: RawStr,
        data: Bytes,
        parent: Option<Parent>,
        meter: &Arc<Meter>,
        ctx: &Arc<FileContext>,
    ) -> Result<Arc<Self>, Error> {
        // The handle itself, in its `Arc`.
        let mut charge = meter.charge(heap_cost(
            u64::try_from(std::mem::size_of::<Self>() + 16).unwrap_or(u64::MAX),
        ))?;
        let mut p = Parser {
            r: Reader::new(&data, Endian::Big),
            data: &data,
            meter,
            limits: &ctx.limits,
            common: ctx.providers.common_strings(),
            version: 0,
            enable_type_tree: true,
            kept: Charge::zero(meter),
        };
        let header = p.header()?;
        p.version = header.version;
        p.r.set_endian(header.endian);
        let format = header.version;
        // UnityPy files/SerializedFile.py:271-273, :346-360
        let (unity_version, version) = if format >= 7 {
            let own = string_to_null(&mut p.r, &data, meter)?;
            let parent_engine = match parent.as_ref().map(|p| &p.kind) {
                Some(ParentKind::Bundle { version_engine, .. }) => Some(version_engine),
                _ => None,
            };
            let resolved = resolve_serialized(&own, parent_engine, &ctx.version, &mut charge)?;
            (Some(own), Some(resolved))
        } else {
            (None, None)
        };
        // :275-280
        let target_platform = if format >= 8 {
            Some(p.r.read_i32()?)
        } else {
            None
        };
        if format >= 13 {
            p.enable_type_tree = p.r.read_bool()?;
        }
        // :282-284
        let mut types = p.types(false)?;
        // :286-288
        let big_id_enabled = if (7..14).contains(&format) {
            p.r.read_i32()?
        } else {
            0
        };
        // :290-295
        let object_infos = p.objects(big_id_enabled, header.data_offset, &mut types)?;
        // :297-300
        let script_types = p.scripts()?;
        // :302-304
        let externals = p.externals()?;
        // :306-308
        let ref_types = if format >= 20 {
            Some(keep(p.types(true)?, &mut charge)?)
        } else {
            None
        };
        // :313-314
        let user_information = if format >= 5 {
            Some(string_to_null(&mut p.r, &data, meter)?)
        } else {
            None
        };
        let enable_type_tree = p.enable_type_tree;
        charge.absorb(p.kept);
        let (object_order, by_path) = dict_order(&object_infos, meter)?;
        let types = keep(types, &mut charge)?;
        let object_infos = keep(object_infos, &mut charge)?;
        let script_types = keep(script_types, &mut charge)?;
        let externals = keep(externals, &mut charge)?;
        let object_order = keep(object_order, &mut charge)?;
        let by_path = keep(by_path, &mut charge)?;
        let file = Arc::new(Self {
            name,
            data,
            header,
            unity_version,
            version,
            target_platform,
            enable_type_tree,
            big_id_enabled,
            types,
            ref_types,
            script_types,
            externals,
            user_information,
            object_infos,
            object_order,
            by_path,
            container: ResultCell::new(),
            parent,
            charge,
            ctx: ctx.clone(),
        });
        // :316-324: the first AssetBundle in dict order is read at load; a
        // failure fails the parse, whose grants the bundle counts.
        if ctx.eager_asset_bundle
            && file
                .object_order
                .iter()
                .filter_map(|i| file.object_infos.get(usize::try_from(*i).ok()?))
                .any(|o| o.class_id == ASSET_BUNDLE)
        {
            file.container_at_load()?;
        }
        Ok(file)
    }
}

/// `ClassIDType.AssetBundle`.
const ASSET_BUNDLE: i32 = 142;

/// A table as the file keeps it, its charge moved into the file's.
fn keep<T>(table: MeteredVec<T>, charge: &mut Charge) -> Result<Box<[T]>, Error> {
    let (b, c) = table.into_boxed_slice()?;
    charge.absorb(c);
    Ok(b)
}

/// The metadata reader of one parse.
struct Parser<'a> {
    r: Reader<'a>,
    data: &'a Bytes,
    meter: &'a Arc<Meter>,
    limits: &'a Limits,
    /// The caller's TPK's common strings, which blob trees read (`None`:
    /// UnityPy's MIT table).
    common: Option<&'a CommonStrings>,
    /// The format (`header.version`).
    version: u32,
    /// `_enable_type_tree` as read so far.
    enable_type_tree: bool,
    /// Charges of what the type records keep outside their node lists.
    kept: Charge,
}

impl Parser<'_> {
    /// `SerializedFileHeader` and the header fields `__init__` reads after it
    /// (`SerializedFile.py:248-269`): big-endian, the endian byte at 16 from
    /// format 9, else at `file_size - metadata_size` (a Python position,
    /// negative when the metadata is larger than the file).
    fn header(&mut self) -> Result<SerializedHeader, Error> {
        let r = &mut self.r;
        let mut metadata_size = r.read_u32()?;
        let mut file_size = i64::from(r.read_u32()?);
        let version = r.read_u32()?;
        let mut data_offset = i64::from(r.read_u32()?);
        let (endian, reserved, unknown);
        let order = |big: bool| if big { Endian::Big } else { Endian::Little };
        if version >= 9 {
            endian = order(r.read_bool()?);
            // `read_bytes(3)`: short at the end without an error; every
            // later read then fails.
            let got = r.read_bytes_short(3);
            let mut three = [0u8; 3];
            for (d, s) in three.iter_mut().zip(got) {
                *d = *s;
            }
            reserved = Some(three);
            if version >= 22 {
                metadata_size = r.read_u32()?;
                file_size = r.read_i64()?;
                data_offset = r.read_i64()?;
                unknown = r.read_i64()?;
            } else {
                unknown = 0;
            }
        } else {
            r.set_position(i128::from(file_size) - i128::from(metadata_size));
            endian = order(r.read_bool()?);
            reserved = None;
            unknown = 0;
        }
        Ok(SerializedHeader {
            metadata_size,
            file_size,
            version,
            data_offset,
            endian,
            reserved,
            unknown,
        })
    }

    /// The script table (`LocalSerializedObjectIdentifier`), format 11 up.
    fn scripts(&mut self) -> Result<MeteredVec<ScriptRef>, Error> {
        let mut scripts = MeteredVec::new(self.meter);
        if self.version < 11 {
            return Ok(scripts);
        }
        for _ in 0..self.r.read_i32()?.max(0) {
            let file_index = self.r.read_i32()?;
            let identifier = if self.version < 14 {
                i64::from(self.r.read_i32()?)
            } else {
                self.r.align(4);
                self.r.read_i64()?
            };
            scripts.push(ScriptRef {
                file_index,
                identifier,
            })?;
        }
        Ok(scripts)
    }

    /// The externals (`FileIdentifier`).
    fn externals(&mut self) -> Result<MeteredVec<External>, Error> {
        let mut externals = MeteredVec::new(self.meter);
        for _ in 0..self.r.read_i32()?.max(0) {
            let temp_empty = if self.version >= 6 {
                Some(string_to_null(&mut self.r, self.data, self.meter)?)
            } else {
                None
            };
            let (guid, kind) = if self.version >= 5 {
                // `read_bytes(16)` kept as it is: short (empty at a position
                // in -16..0) without an error, as UnityPy keeps it.
                (Some(self.r.read_short_array()), Some(self.r.read_i32()?))
            } else {
                (None, None)
            };
            let path = string_to_null(&mut self.r, self.data, self.meter)?;
            externals.push(External {
                temp_empty,
                guid,
                kind,
                path,
            })?;
        }
        Ok(externals)
    }

    /// `[SerializedType(reader, self, is_ref_type) for _ in range(count)]`.
    fn types(&mut self, is_ref: bool) -> Result<MeteredVec<SerializedType>, Error> {
        let mut types = MeteredVec::new(self.meter);
        for _ in 0..self.r.read_i32()?.max(0) {
            let t = self.serialized_type(is_ref)?;
            types.push(t)?;
        }
        Ok(types)
    }

    /// `SerializedType.__init__` (`SerializedFile.py:110-148`).
    fn serialized_type(&mut self, is_ref: bool) -> Result<SerializedType, Error> {
        let v = self.version;
        let class_id = self.r.read_i32()?;
        let mut t = SerializedType::new(class_id, None);
        if v >= 16 {
            t.is_stripped_type = Some(self.r.read_bool()?);
        }
        if v >= 17 {
            t.script_type_index = self.r.read_i16()?;
        }
        if v >= 13 {
            if (is_ref && t.script_type_index >= 0)
                || (v < 16 && class_id < 0)
                || (v >= 16 && class_id == 114)
            {
                t.script_id = Some(self.r.read_short_array());
            }
            t.old_type_hash = Some(self.r.read_short_array());
        }
        if self.enable_type_tree {
            let rows = if is_blob_format(v) {
                parse_blob(
                    &mut self.r,
                    self.data,
                    v,
                    self.meter,
                    self.limits,
                    self.common,
                )?
            } else {
                parse_old(&mut self.r, self.data, v, self.meter, self.limits)?
            };
            t.nodes = Some(Arc::new(NodeList::from_rows(rows)?));
            if v >= 21 {
                if is_ref {
                    t.class_name = Some(string_to_null(&mut self.r, self.data, self.meter)?);
                    t.namespace = Some(string_to_null(&mut self.r, self.data, self.meter)?);
                    t.assembly = Some(string_to_null(&mut self.r, self.data, self.meter)?);
                } else {
                    let deps = self.r.read_i32_array(self.meter)?;
                    let (deps, c) = deps.into_boxed_slice()?;
                    // The file keeps the list's charge.
                    self.kept.absorb(c);
                    t.type_dependencies = Some(deps);
                }
            }
        }
        Ok(t)
    }

    /// The object table (`ObjectReader.from_reader`, `ObjectReader.py:67-130`),
    /// writing script indexes back into `types` at 11-16. Below 16 each
    /// object's type is found through [`TypesByClass`].
    fn objects(
        &mut self,
        big_id_enabled: i32,
        data_offset: i64,
        types: &mut MeteredVec<SerializedType>,
    ) -> Result<MeteredVec<ObjectInfo>, Error> {
        let v = self.version;
        let mut objects = MeteredVec::new(self.meter);
        let mut by_class: Option<TypesByClass> = None;
        for _ in 0..self.r.read_i32()?.max(0) {
            let r = &mut self.r;
            let path_id = if big_id_enabled != 0 {
                r.read_i64()?
            } else if v < 14 {
                i64::from(r.read_i32()?)
            } else {
                r.align(4);
                r.read_i64()?
            };
            let stored = if v >= 22 {
                i128::from(r.read_i64()?)
            } else {
                i128::from(r.read_u32()?)
            };
            let byte_start = stored + i128::from(data_offset);
            let byte_size = r.read_u32()?;
            let type_id = r.read_i32()?;
            let (class_id, type_index) = if v < 16 {
                let class_id = i32::from(r.read_u16()?);
                // A script index written back below changes no class id, so
                // the index built on the first object holds for every one.
                let by_class = match &mut by_class {
                    Some(t) => t,
                    none => none.insert(TypesByClass::new(types.as_slice(), self.meter)?),
                };
                (class_id, by_class.first(type_id))
            } else {
                // `types[type_id]`: Python indexing.
                let len = types.len();
                let index = if type_id < 0 {
                    len.checked_sub(usize::try_from(type_id.unsigned_abs()).unwrap_or(usize::MAX))
                } else {
                    usize::try_from(type_id).ok().filter(|i| *i < len)
                };
                let index = index.ok_or(Error::TypeIndex {
                    index: type_id,
                    len,
                })?;
                let class_id = types.as_slice().get(index).map_or(0, |t| t.class_id);
                (class_id, Some(index))
            };
            let is_destroyed = if v < 11 { Some(r.read_u16()?) } else { None };
            let script_type_index = if (11..17).contains(&v) {
                let i = r.read_i16()?;
                if let Some(t) = type_index.and_then(|t| types.as_mut_slice().get_mut(t)) {
                    t.script_type_index = i;
                }
                Some(i)
            } else {
                None
            };
            let is_stripped = if v == 15 || v == 16 {
                Some(r.read_i8()?)
            } else {
                None
            };
            objects.push(ObjectInfo {
                path_id,
                byte_start,
                byte_size,
                type_id,
                class_id,
                type_index,
                is_destroyed,
                is_stripped,
                script_type_index,
            })?;
        }
        Ok(objects)
    }
}

/// The types by class id, below format 16, where an object's type is the
/// first type whose class id is the object's stored type id
/// (`ObjectReader.py:91-97`, a scan of `types` per object): `(class id,
/// index)` pairs sorted, built once, on the first object, in a table charged
/// to the input's meter whose bytes count as the open's work, as a decode's
/// output does. A lookup is a binary search, so O objects over T types cost
/// O log T steps, not the O x T of a scan each (round 4's sweep).
struct TypesByClass {
    pairs: MeteredVec<(i32, u32)>,
}

impl TypesByClass {
    fn new(types: &[SerializedType], meter: &Arc<Meter>) -> Result<Self, Error> {
        let bytes = types
            .len()
            .saturating_mul(std::mem::size_of::<(i32, u32)>());
        meter.count_decode(u64::try_from(bytes).unwrap_or(u64::MAX))?;
        let mut pairs = MeteredVec::with_capacity(meter, types.len())?;
        for (i, t) in types.iter().enumerate() {
            // The table is a `MeteredVec` of an `i32` count's rows.
            pairs.push((t.class_id, u32::try_from(i).unwrap_or(u32::MAX)))?;
        }
        pairs.as_mut_slice().sort_unstable();
        Ok(Self { pairs })
    }

    /// The index of the first type of class `class_id`.
    fn first(&self, class_id: i32) -> Option<usize> {
        let pairs = self.pairs.as_slice();
        let at = pairs.partition_point(|(c, _)| *c < class_id);
        pairs
            .get(at)
            .filter(|(c, _)| *c == class_id)
            .and_then(|(_, i)| usize::try_from(*i).ok())
    }
}

/// The `objects` dict (`self.objects[obj.path_id] = obj`): each path id at
/// its first position, holding its last row; and the same entries sorted by
/// path id. Both computed by sorting, in tables charged to `meter`.
fn dict_order(
    infos: &MeteredVec<ObjectInfo>,
    meter: &Arc<Meter>,
) -> Result<(MeteredVec<u32>, MeteredVec<u32>), Error> {
    let rows = infos.as_slice();
    let n = rows.len();
    let index = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
    let path = |i: &u32| {
        usize::try_from(*i)
            .ok()
            .and_then(|i| rows.get(i))
            .map_or(i64::MIN, |o| o.path_id)
    };
    let mut perm: MeteredVec<u32> = MeteredVec::with_capacity(meter, n)?;
    for i in 0..n {
        perm.push(index(i))?;
    }
    // Equal path ids stay in file order (the index breaks the tie), sorted
    // in place: a stable sort's scratch would be heap no meter charged
    // (plan 3.15).
    perm.as_mut_slice().sort_unstable_by_key(|i| (path(i), *i));
    let mut by_path: MeteredVec<u32> = MeteredVec::new(meter);
    let mut groups: MeteredVec<(u32, u32)> = MeteredVec::new(meter);
    let mut at = 0;
    let all = perm.as_slice();
    while let Some(first) = all.get(at) {
        let key = path(first);
        let mut last = *first;
        let mut end = at + 1;
        while let Some(next) = all.get(end).filter(|i| path(i) == key) {
            last = *next;
            end += 1;
        }
        by_path.push(last)?;
        groups.push((*first, last))?;
        at = end;
    }
    drop(perm);
    groups.as_mut_slice().sort_unstable_by_key(|g| g.0);
    let mut order: MeteredVec<u32> = MeteredVec::with_capacity(meter, groups.len())?;
    for (_, last) in groups.as_slice() {
        order.push(*last)?;
    }
    Ok((order, by_path))
}
