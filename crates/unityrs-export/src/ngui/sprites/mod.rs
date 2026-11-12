// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/SpriteHelper.py's get_image and get_image_from_sprite (which Sprites read and render, the sheet and alpha texture each takes, in that order), and export/Texture2DConverter.py's parse_image_data for a side of 0. See THIRD_PARTY_LICENSES.
//! The `sprites` output kind (`outputs.sprites`, `docs/BATCH_SPEC.md` 1.5;
//! `docs/plans/extract.md` E17): one PNG per Sprite of the input's own
//! `SerializedFile`s (section 3), rendered by `unityrs_texture::sprite` from
//! a sheet that may be in a dependency (`docs/plans/batch.md` 2.6).
//!
//! - **The plan** ([`plan`], 4.4, in plan order between the textures kind
//!   and the atlases): each Sprite's name (step 1, `duplicate_name` among
//!   Sprites), the skips that need no pixels (step 2: the Sprite's checked
//!   read as UnityPy constructs it, then E17's fields ([`object`]), its
//!   render data ([`atlas`]), then its texture's dereference, object read
//!   and, for a sheet with no side of 0, section 3's answers 2 and 3, and
//!   then its alpha texture's), then its key from the `sprites` template
//!   (step 3), from the attempt's one key table.
//! - **Sprite groups** (4.4): the planned Sprites grouped by their sheet,
//!   the texture's file ([`files`]: the row chain it was opened from) and
//!   path id, and the alpha texture's or none ([`SheetId`]), in the order
//!   of each group's first Sprite (a counting sort, linear and counted),
//!   whose pointers to its sheet the group keeps ([`GroupSheet`]).
//! - **The export** ([`export`]), first in 4.4's export order: each group's
//!   sheet dereferenced from its record and decoded once (section 3's
//!   answers 4 to 10, 12 and 13; a side of 0 is UnityPy's 0 x 0 `RGBA`
//!   image; a failure every Sprite's of the group), its alpha texture with
//!   it and merged in (`sprite::Sheet`), then each Sprite rendered, in plan
//!   order, and its PNG made (E19) with its visible hash (E20).
//!
//! The textures a Sprite renders from are found by dereferencing, so every
//! such read is an Environment call checked as 2.6 step 7 says, its target's
//! top-level bundle held while it is used and dropped when the read ends:
//! the plan keeps no handle into another bundle (a group keeps its first
//! Sprite's own file and pointers, an atlas's index keeps render data, not
//! the atlas). Every buffer is charged to `M_X` before it is allocated,
//! every core call is under the refusal probe on its own, and each pixel
//! buffer is counted as export work (`docs/plans/batch.md` 2.6 step 10): a
//! decoded sheet's and a merged sheet's before they are made, a rendered
//! image's once the render returns it, after answer 11 and before its PNG
//! (the canvases inside the render are charged to `M_X` and its loops
//! counted as decode work, E59).

mod atlas;
mod export;
mod files;
mod object;
mod plan;

use std::sync::Arc;

use unityrs_core::ObjectRef;
use unityrs_core::env::PPtr;
use unityrs_core::files::{BundleFile, SerializedFile};
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_texture::sprite::{SpriteData, SpriteError};

use super::MemberPath;
use super::classify::{self, Class, Reason};
use super::read;
use super::seam::Seam;
use super::table::Map;
use object::{Ids, RenderData};

/// `Sprite`'s class id.
pub(super) const SPRITE: i32 = 213;
/// `SpriteAtlas`'s class id.
pub(super) const SPRITE_ATLAS: i32 = 687_078_895;

impl From<SpriteError> for Class {
    fn from(e: SpriteError) -> Self {
        classify::render(&e)
    }
}

/// A texture's identity in 4.4's sprite groups: its file's number in the
/// attempt ([`files::Files`]) and its path id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct SheetId {
    file: u32,
    path_id: i64,
}

/// A sprite group's sheet: the texture's identity, and its alpha
/// texture's or none.
pub(super) type GroupKey = (SheetId, Option<SheetId>);

/// Where a group's export finds its sheet (recorded at the plan, from the
/// group's first Sprite): the Sprite's own file; the `m_SpriteAtlas`
/// pointer of it, where its render data is that atlas's (the textures'
/// pointers are then the atlas's file's); and the texture's and alpha
/// texture's pointers.
pub(super) struct GroupSheet {
    from: Arc<SerializedFile>,
    atlas: Option<PPtr>,
    texture: PPtr,
    alpha: Option<PPtr>,
}

/// A planned Sprite: its object, what it renders from, its group and its
/// key.
pub(super) struct SpritePlan {
    /// The Sprite.
    pub obj: ObjectRef,
    /// Its render data's rectangle and settings, with its
    /// `m_PixelsToUnits`.
    pub data: SpriteData,
    /// Its group's index in [`Sprites::groups`].
    pub group: u32,
    /// Its key.
    pub key: String,
    /// Its archive member paths.
    pub members: MeteredVec<MemberPath>,
}

/// The sprites kind's plan: the planned Sprites in plan order, and their
/// groups' sheets in the order of each one's first Sprite.
pub(super) struct Sprites {
    /// The planned Sprites, in plan order.
    pub planned: MeteredVec<SpritePlan>,
    /// The groups' sheets.
    pub groups: MeteredVec<GroupSheet>,
    /// Each group's index, by its key.
    index: Map<GroupKey, u32>,
}

impl Sprites {
    /// No Sprite planned.
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            planned: MeteredVec::new(meter),
            groups: MeteredVec::new(meter),
            index: Map::new(meter),
        }
    }
}

/// A Sprite's render data, resolved (E17): its rectangle and settings
/// with its `m_PixelsToUnits`; the render data; the file its pointers are
/// read from; the `m_SpriteAtlas` pointer, where the render data is that
/// atlas's; and the atlas's top-level bundle, held while the caller uses
/// them (2.6 step 7).
pub(super) struct Resolved {
    /// The render data's rectangle and settings, and `m_PixelsToUnits`.
    pub data: SpriteData,
    /// The render data.
    rd: RenderData,
    /// The file that holds it.
    pub holder: Arc<SerializedFile>,
    /// `m_SpriteAtlas`, where the render data is its atlas's.
    pub atlas: Option<PPtr>,
    _held: Option<Arc<BundleFile>>,
}

fn bad_pointer() -> Class {
    Class::Skip(Reason::plain("bad_pointer"))
}

/// A `PPtr`'s ids followed from `from` (`PPtr.deref`), with the core
/// pointer they are: an `m_FileID` outside an `int` names no external,
/// `bad_pointer` (the core's `FileIdPastEnd`); an `m_PathID` outside an
/// `i64` (a `UInt64` past `2^63`) names no object of any file, so its file
/// is found as any id's is (its failure the pointer's) and then none is
/// there, `bad_pointer` (UnityPy's `KeyError`, the core's `MissingObject`;
/// B3's review round 3, S3a).
fn follow(
    seam: &Seam<'_>,
    from: &Arc<SerializedFile>,
    ids: Ids,
) -> Result<(ObjectRef, PPtr), Class> {
    let file_id = i32::try_from(ids.file_id).map_err(|_| bad_pointer())?;
    let Ok(path_id) = i64::try_from(ids.path_id) else {
        let any = PPtr {
            file_id,
            path_id: 1,
        };
        drop(read::deref(seam, from, any)?);
        return Err(bad_pointer());
    };
    let pptr = PPtr { file_id, path_id };
    Ok((read::deref(seam, from, pptr)?, pptr))
}
