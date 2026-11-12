// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/SpriteHelper.py's get_image_from_sprite (its atlas lookup). See THIRD_PARTY_LICENSES.
//! A Sprite's render data (E17; 4.4 step 2), as `get_image_from_sprite`
//! finds it (`export/SpriteHelper.py:63-81`), with the work an attempt of
//! many Sprites does bounded (B3's review rounds 1 to 3, R2, S4 and S8):
//!
//! - a true `m_SpriteAtlas` (`export/SpriteHelper.py:64`, a `PPtr`'s truth
//!   its path id's) is dereferenced from the Sprite's file (a failure its
//!   `DerefError`'s class; a value that is no `PPtr` raises, `render`) and
//!   the atlas read, whatever class UnityPy constructs it as, but a class
//!   that cannot take its reader ([`super::object::attaches`], `read`);
//! - else, where `m_AtlasTags` is true, the `SpriteAtlas` objects of the
//!   Sprite's file are listed once per attempt, in object order (each
//!   object walked one unit of decode work), and taken in that order up to
//!   the first whose name equals the tag, which is read: each name read
//!   once, when a walk first reaches its place, and indexed, its first
//!   place under its bytes ([`List`]), so a tag is one lookup of the names
//!   read so far (one unit and its bytes) and a walk goes on from where the
//!   last stopped, each name it reads one unit and its bytes, and its
//!   comparison with the tag one unit and the shorter's bytes (B3's review
//!   round 4, R1); one with no name field, or a name that is no
//!   string, is no match (`peek_name`'s `None`); a name's read that fails
//!   is kept at its place, and is the failure of each Sprite whose tag no
//!   earlier name equals; none matching is no atlas. A first tag that is
//!   no string matches none, so every name is read; a true `m_AtlasTags`
//!   that is not indexable raises at the first atlas, after its name's read
//!   (`render`), whatever a later place's kept failure is (round 4, R3);
//! - with no atlas, the Sprite's own `m_RD`.
//!
//! An atlas is read once per attempt, by its identity, and its
//! `m_RenderDataMap` walked once ([`super::object::walk`]) and indexed by
//! key, the first entry of a key (so K Sprites on it cost K lookups); each
//! Sprite's entry is its key's (`next(...)`, `:78`), none `render`
//! (`StopIteration`, or the walk's own failure past the entries it
//! indexed). Each object's failure kept by these tables is a [`Kept`]; a
//! refusal of `M_X`'s is the Sprite's and not kept (B3's review round 2,
//! S1), and a name's charge is kept only once its entry is in (round 3,
//! S7), its copy made only once that charge is granted (round 4, R2).

use std::sync::Arc;

use unityrs_core::ObjectRef;
use unityrs_core::files::{BundleFile, SerializedFile};
use unityrs_core::limits::{Charge, Meter, MeteredVec};

use super::super::attempt::Attempt;
use super::super::classify::{self, Class, Reason};
use super::super::kept::Kept;
use super::super::read::{self, top_bundle};
use super::super::table::{self, Map};
use super::files::Files;
use super::object::{self, Item, Key, Ptr, RenderData, SpriteRead, Tag};
use super::{SPRITE_ATLAS, follow};

/// A file's atlases (B3's review round 3, S8): their objects' indices in
/// object order; the first place of each name read so far; how many
/// places have had their names read (each a name, or none); and the
/// failure of the next place's read, which stops every walk there.
struct List {
    objects: MeteredVec<u32>,
    names: Map<Vec<u8>, usize>,
    read: usize,
    failed: Option<Kept>,
    held: Charge,
}

/// A tag walk's outcome.
#[derive(Clone, Copy)]
enum Walked {
    /// The atlas at this object index.
    Atlas(u32),
    /// None matched.
    NoAtlas,
    /// `m_AtlasTags[0]` raised.
    Raised,
}

/// What a walk compares names with: a tag, nothing a name equals, or an
/// index that raises (`Tag`'s).
#[derive(Clone, Copy)]
enum Want<'t> {
    Name(&'t [u8]),
    Nothing,
    Raise,
}

/// An atlas read and indexed: each key's first entry (its render data, or
/// the value's failure) and the failure that ended the walk, if one did.
struct Index {
    entries: Map<Key, Result<RenderData, Kept>>,
    tail: Option<Kept>,
}

/// The attempt's atlas tables and file identities.
pub(super) struct Atlases {
    pub files: Files,
    lists: Map<u32, List>,
    read: Map<(u32, i64), Result<Index, Kept>>,
}

impl Atlases {
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            files: Files::new(meter),
            lists: Map::new(meter),
            read: Map::new(meter),
        }
    }
}

/// The render data a Sprite renders from; the file its pointers are read
/// from (the atlas's, or the Sprite's); the `m_SpriteAtlas` pointer, where
/// its atlas is that one; and the atlas's top-level bundle, held while they
/// are.
pub(super) type Found = (
    RenderData,
    Arc<SerializedFile>,
    Option<unityrs_core::env::PPtr>,
    Option<Arc<BundleFile>>,
);

/// The work of looking up or indexing a name (`unityrs_texture`'s own
/// measure of a comparison): one unit, and one more a byte.
fn hashed(name: &[u8]) -> u64 {
    1 + u64::try_from(name.len()).unwrap_or(u64::MAX)
}

fn internal() -> Class {
    Class::Run(Reason::plain("internal"))
}

fn counted(meter: &Meter, n: u64) -> Result<(), Class> {
    meter.count_decode(n).map_err(|e| classify::charge(&e))
}

impl Attempt<'_> {
    /// E17's render data for `obj`'s Sprite (module docs).
    pub(super) fn render_data(
        &mut self,
        obj: &ObjectRef,
        sprite: &SpriteRead<'_>,
        at: &mut Atlases,
    ) -> Result<Found, Class> {
        let seam = self.seam;
        let meter = seam.meter;
        match sprite.atlas.filter(|p| p.is_set()) {
            // `.deref_parse_as_object()` of a value that is no `PPtr`.
            Some(Ptr::Other(_)) => return Err(object::render()),
            Some(Ptr::Ids(ids)) => {
                let (atlas, pptr) = follow(&seam, obj.file(), ids)?;
                let held = top_bundle(atlas.file());
                let id = seam.probe(|| at.files.id(atlas.file(), meter))?;
                let rd = self.atlas_data(&atlas, id, sprite.key, at)?;
                return Ok((rd, atlas.file().clone(), Some(pptr), held));
            }
            None => {}
        }
        let own = obj.file().clone();
        let want = match sprite.tag {
            Tag::None => return Ok((sprite.rd, own, None, None)),
            Tag::Name(t) => Want::Name(t),
            Tag::NoMatch => Want::Nothing,
            Tag::Raises => Want::Raise,
        };
        let id = seam.probe(|| at.files.id(&own, meter))?;
        match self.walk(&own, id, want, at)? {
            Walked::Atlas(index) => {
                let atlas = ObjectRef::new(own.clone(), index).ok_or_else(internal)?;
                let rd = self.atlas_data(&atlas, id, sprite.key, at)?;
                Ok((rd, own, None, None))
            }
            Walked::NoAtlas => Ok((sprite.rd, own, None, None)),
            Walked::Raised => Err(object::render()),
        }
    }

    /// File `file`'s atlases, listed once.
    fn listed(&self, own: &Arc<SerializedFile>, file: u32, at: &mut Atlases) -> Result<(), Class> {
        if at.lists.get(&file).is_some() {
            return Ok(());
        }
        let seam = self.seam;
        let meter = seam.meter;
        let mut objects = MeteredVec::new(meter);
        for o in own.objects() {
            counted(meter, 1)?;
            if o.class_id() == SPRITE_ATLAS {
                seam.probe(|| table::push(&mut objects, o.index()))?;
            }
        }
        let list = List {
            objects,
            names: Map::new(meter),
            read: 0,
            failed: None,
            held: Charge::zero(meter),
        };
        let lists = &mut at.lists;
        seam.probe(|| lists.insert(file, list))
    }

    /// The tag walk over file `file`'s atlases for `want` (module docs): the
    /// names read so far looked up once, then the walk taken on from the
    /// first place not read, each name read compared with the tag and
    /// indexed, up to a match, a failure or the end. A name's read that
    /// fails is the Sprite's failure, and kept. A tag that raises does so
    /// at the first place whose name was read, before any later place's
    /// kept failure, as UnityPy's loop raises there (B3's review round 4,
    /// R3); each lookup and comparison is counted (R1).
    fn walk(
        &mut self,
        own: &Arc<SerializedFile>,
        file: u32,
        want: Want<'_>,
        at: &mut Atlases,
    ) -> Result<Walked, Class> {
        let seam = self.seam;
        let meter = seam.meter;
        self.listed(own, file, at)?;
        if let Want::Name(t) = want {
            counted(meter, hashed(t))?;
            let list = at.lists.get(&file).ok_or_else(internal)?;
            if let Some(p) = list.names.get(t) {
                let index = list.objects.as_slice().get(*p).ok_or_else(internal)?;
                return Ok(Walked::Atlas(*index));
            }
        }
        loop {
            let list = at.lists.get_mut(&file).ok_or_else(internal)?;
            if matches!(want, Want::Raise) && list.read > 0 {
                return Ok(Walked::Raised);
            }
            if let Some(k) = &list.failed {
                return Err(k.class());
            }
            let place = list.read;
            let Some(index) = list.objects.as_slice().get(place).copied() else {
                return Ok(Walked::NoAtlas);
            };
            counted(meter, 1)?;
            let o = ObjectRef::new(own.clone(), index).ok_or_else(internal)?;
            let mut matched = false;
            match self.reader.atlas_name(&seam, &o) {
                Ok(Some(name)) => {
                    let name = name.as_bytes();
                    // Its lookup (and insert), one unit and its bytes; and
                    // its comparison with a tag, one unit and the bytes the
                    // shorter has. A name equal to the tag is new: the tag
                    // was in no place read before this walk, nor in one
                    // this walk read.
                    counted(meter, hashed(name))?;
                    if let Want::Name(t) = want {
                        counted(meter, hashed(name.get(..t.len()).unwrap_or(name)))?;
                        #[cfg(feature = "test-counters")]
                        crate::testing::tag_walk(name.len().min(t.len()));
                        matched = t == name;
                    }
                    if list.names.get(name).is_none() {
                        let cost = table::string(name.len());
                        let (names, held) = (&mut list.names, &mut list.held);
                        let copy = || table::copied(name);
                        seam.probe(|| names.insert_held(held, cost, copy, place))?;
                    }
                }
                Ok(None) => {}
                // A refusal of `M_X`'s is the Sprite's: nothing is kept, and
                // the place is read again by the next walk.
                Err(c) => {
                    list.failed = Some(Kept::keep(meter, seam.hooks, c)?);
                    continue;
                }
            }
            list.read = place + 1;
            if matched {
                return Ok(Walked::Atlas(index));
            }
        }
    }

    /// The render data of `atlas` (of file number `file`) for `key`: the
    /// atlas read and indexed once, then its key's entry.
    fn atlas_data(
        &self,
        atlas: &ObjectRef,
        file: u32,
        key: Option<Key>,
        at: &mut Atlases,
    ) -> Result<RenderData, Class> {
        let seam = self.seam;
        let id = (file, atlas.path_id());
        if at.read.get(&id).is_none() {
            let built = self.index(atlas)?;
            let read = &mut at.read;
            seam.probe(|| read.insert(id, built))?;
        }
        let index = match at.read.get(&id) {
            Some(Ok(index)) => index,
            Some(Err(k)) => return Err(k.class()),
            None => return Err(internal()),
        };
        match key.as_ref().and_then(|k| index.entries.get(k)) {
            Some(Ok(rd)) => Ok(*rd),
            Some(Err(k)) => Err(k.class()),
            None => Err(index.tail.as_ref().map_or_else(object::render, Kept::class)),
        }
    }

    /// `atlas` read (checked, 4.4 step 2, its reader attached) and indexed,
    /// or its failure kept; a refusal of `M_X`'s, or the input's or the
    /// run's failure, is returned, not kept.
    fn index(&self, atlas: &ObjectRef) -> Result<Result<Index, Kept>, Class> {
        let seam = self.seam;
        let meter = seam.meter;
        let hooks = seam.hooks;
        let parsed = match seam.probe(|| {
            atlas
                .read_typetree(None, true)
                .map_err(|e| classify::parse(&e))
                .and_then(|p| {
                    if object::attaches(&p, meter)? {
                        Ok(p)
                    } else {
                        Err(read::shape())
                    }
                })
        }) {
            Ok(p) => p,
            Err(c) => return Ok(Err(Kept::keep(meter, hooks, c)?)),
        };
        let mut entries: Map<Key, Result<RenderData, Kept>> = Map::new(meter);
        let walked = seam.probe(|| {
            object::walk(&parsed, meter, |item| {
                let Item::Entry(Some(k), v) = item else {
                    return Ok(true);
                };
                if entries.get(&k).is_some() {
                    return Ok(true);
                }
                let value = match object::atlas_data(&parsed, v, meter) {
                    Ok(rd) => Ok(rd),
                    Err(c) => Err(Kept::keep(meter, hooks, c)?),
                };
                entries.insert(k, value)?;
                Ok(true)
            })
        });
        let tail = match walked {
            Ok(None) => None,
            Ok(Some(c)) => Some(Kept::keep(meter, hooks, c)?),
            Err(c) => return Ok(Err(Kept::keep(meter, hooks, c)?)),
        };
        Ok(Ok(Index { entries, tail }))
    }
}
