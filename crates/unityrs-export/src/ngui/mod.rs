//! The NGUI atlas cutter (`docs/plans/extract.md` E21 to E26, X5;
//! `docs/BATCH_SPEC.md` sections 3, 4.3, 4.4 and 5), over one input's view.
//!
//! The cutter is one phase of an attempt the engine owns
//! (`docs/plans/batch.md` 2.6; [`seam`]). The engine registers the view
//! ([`register`]: X's own open on `M_X`, the private Environment, each
//! bundle's marked rows reached) and hands [`cut`] a [`Context`]: the
//! registered view, `M_X`, its hooks (the stop flag, the refusal count,
//! export work), the sink's prefix, the archives' member templates, the key
//! table and the sink. [`cut`] opens every top-level row of X's own handle
//! (section 3's selection), plans (4.4: names, the skips that need no
//! pixels, keys, in the textures, sprites, atlases order) and exports
//! (sheet groups, then the other textures), handing each output to the
//! sink as it is made, and returns its skips (the plan's and the
//! export's apart), sheets and counts. Its reasons are 9.3's, its details
//! 7.3's, classified as 2.7's `classify` does ([`classify`]). [`winners`]
//! decides 4.5's steps 1 to 3 over several inputs' cuts.
//!
//! Not here, being the engine's: staging, the peek pass, the attempt's
//! budget and its counts, the spool and a file's export work, the
//! `sprites` kind (E17, whose stage has its slot), the records' files and
//! 4.6.

mod attempt;
pub mod classify;
mod export;
pub mod keys;
mod plan;
pub mod read;
pub mod recognize;
pub mod register;
mod resource;
pub mod seam;
mod table;
pub mod winners;

use unityrs_core::limits::Charge;
use unityrs_core::{Bytes, RawStr};

pub use classify::{Class, Reason};
pub use keys::{KeyRule, KeyTable, Template, TemplateError, TemplateKind};
pub use register::{
    Admit, Bits, Data, PeekBits, Registration, Staged, peek, register, register_bytes, same_open,
};
pub use seam::{Context, Hooks, MemberTemplate, NoHooks, Registered};
pub use winners::{Candidate, Decision, Prefer, Rule, decide};

/// What section 1.5's `atlases` (and `outputs.textures`) ask for.
#[derive(Clone, Debug)]
pub struct Options {
    /// `atlases.sprite_key`.
    pub sprite_key: Template,
    /// `atlases.json_key`.
    pub json_key: Template,
    /// `outputs.textures.key`, when textures are exported.
    pub texture_key: Option<Template>,
    /// `atlases.script_classes`.
    pub script_classes: Vec<Vec<u8>>,
    /// `atlases.skip_name_suffixes`.
    pub skip_name_suffixes: Vec<Vec<u8>>,
    /// Whether atlases are cut (`atlases` present in the spec): without
    /// it, the plan's atlas stage does not run.
    pub atlases: bool,
}

/// A bundle of the view: its spec id and its bytes.
#[derive(Clone, Debug)]
pub struct Member {
    /// Its id.
    pub id: String,
    /// Its bytes.
    pub data: Bytes,
}

/// One input's view (4.2): the input, and every other bundle of the view.
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    /// The input, `X`.
    pub input: &'a Member,
    /// `view(X)` less `X`, in any order.
    pub members: &'a [Member],
}

/// An output's kind (7.3's `outputs.kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OutputKind {
    /// `texture`.
    Texture,
    /// `atlas_json`.
    AtlasJson,
    /// `atlas_sprite`.
    AtlasSprite,
}

impl OutputKind {
    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::AtlasJson => "atlas_json",
            Self::AtlasSprite => "atlas_sprite",
        }
    }
}

/// What a skip skips (7.3's `skips.output`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SkipOutput {
    /// `texture`.
    Texture,
    /// `atlas_json`.
    AtlasJson,
    /// `atlas_sprite`.
    AtlasSprite,
    /// `atlas`: an atlas's JSON and crops together.
    Atlas,
}

impl SkipOutput {
    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::AtlasJson => "atlas_json",
            Self::AtlasSprite => "atlas_sprite",
            Self::Atlas => "atlas",
        }
    }
}

/// A PNG output's image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    /// Its mode: `RGB` or `RGBA`.
    pub mode: &'static str,
    /// Its width.
    pub width: u32,
    /// Its height.
    pub height: u32,
    /// E20's visible hash.
    pub visible_hash: [u8; 32],
}

/// An archive member path an output's variables expand to (4.7): its
/// archive's index, and the path or why it is left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberPath {
    /// The archive's index.
    pub archive: usize,
    /// The path, or why it is left out (`unwritable_path(<rule>)`).
    pub path: Result<String, Rejected>,
}

/// A member path 2.3's rules leave out, as 2.8's `archive_members` row
/// records it: the rule, and the path as expanded (at most its first 1,025
/// bytes) under 7.3's text rule: cut to 1,024 bytes at a code point
/// boundary, each invalid byte U+FFFD, and whether it was cut (review
/// round 2's S5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    /// The rule it breaks.
    pub rule: KeyRule,
    /// The path, under the text rule.
    pub path: String,
    /// Whether the text rule cut it.
    pub cut: bool,
}

/// An output the cut made, handed to the sink as it is made.
#[derive(Debug)]
pub struct Output {
    /// Its key.
    pub key: String,
    /// Its kind.
    pub kind: OutputKind,
    /// Its unit key (4.5), or none.
    pub unit: Option<String>,
    /// Its object's SerializedFile, by its stored name.
    pub serialized_file: RawStr,
    /// Its object's path id (an atlas's outputs, the MonoBehaviour's).
    pub path_id: i64,
    /// An atlas sprite's index in `mSprites`.
    pub sprite: Option<usize>,
    /// The file: a PNG, or the atlas JSON.
    pub bytes: Vec<u8>,
    /// The bytes' MD5.
    pub md5: [u8; 16],
    /// A PNG's image; none for an atlas JSON.
    pub picture: Option<Picture>,
    /// Its archive member paths, expanded at the plan.
    pub members: Vec<MemberPath>,
    /// What `bytes`, its unit key and member paths are charged as, on the
    /// input's meter (E59), returned when the sink drops it.
    _charge: Option<Charge>,
}

/// A skip (7.3's `skips` row, but its input).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skip {
    /// The object's SerializedFile.
    pub serialized_file: RawStr,
    /// The object's path id.
    pub path_id: i64,
    /// The object's class: UnityPy 1.25.2's `ClassIDType` name.
    pub class: String,
    /// What it skips.
    pub output: SkipOutput,
    /// An atlas sprite's index.
    pub sprite: Option<usize>,
    /// Why.
    pub reason: Reason,
}

/// How an atlas's sheet was found (section 5 step 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetBy {
    /// By its material's main-texture name.
    MaterialName,
    /// By the atlas's own name.
    AtlasName,
}

/// An atlas whose sheet was found: the atlas, how, and the texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetRow {
    /// The atlas's SerializedFile.
    pub atlas_file: RawStr,
    /// The atlas's path id.
    pub atlas_path_id: i64,
    /// How the texture was found.
    pub by: SheetBy,
    /// The texture's SerializedFile.
    pub texture_file: RawStr,
    /// The texture's path id.
    pub texture_path_id: i64,
}

/// What one input's cut records beside its outputs, which went to the
/// sink.
#[derive(Debug, Default)]
pub struct Cut {
    /// The plan's skips, in plan order.
    pub plan_skips: Vec<Skip>,
    /// The export's skips, in export order; record order is the plan's,
    /// then these.
    pub export_skips: Vec<Skip>,
    /// `MonoBehaviour`s whose `m_Script` read failed (section 5).
    pub unresolved_scripts: u64,
    /// Every atlas whose sheet was found, in plan order.
    pub sheets: Vec<SheetRow>,
    /// The input's nested containers (`nested_container`), in directory
    /// order.
    pub nested_containers: Vec<RawStr>,
    /// What these records are charged as on `M_X` (their tables and
    /// strings), returned when the cut is dropped (review round 2's S3).
    _charge: Option<Charge>,
}

/// Why the input failed, or the run, or that the attempt stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The input fails with this reason.
    Input(Reason),
    /// The run fails with this reason.
    Run(Reason),
    /// The run stopped (2.11): the attempt returned at its next check, its
    /// end message `Stopped`.
    Stopped,
}

impl From<Class> for Failure {
    /// A class as an input's failure: an object-level reason met where an
    /// object's skip is not the site (an open, a load) fails the input.
    fn from(c: Class) -> Self {
        match c {
            Class::Skip(r) | Class::Input(r) => Self::Input(r),
            Class::Run(r) => Self::Run(r),
        }
    }
}

/// Cuts one input inside the engine's attempt (`ctx`): selects over X's
/// own handle, plans and exports, every output to the sink. A failure the
/// site rules make the input's, or the run's, or a stop, ends the cut.
pub fn cut(ctx: Context<'_>, options: &Options) -> Result<Cut, Failure> {
    let Context {
        registered,
        meter,
        input,
        hooks,
        max_evictions,
        prefix,
        members,
        keys,
        sink,
    } = ctx;
    let seam = seam::Seam {
        registered,
        meter,
        hooks,
        max_evictions,
        prefix,
        members,
    };
    seam.stop()?;
    let mut attempt = attempt::Attempt::new(seam, options, input, keys, sink)?;
    let files = attempt.select()?;
    let planned = attempt.plan(files.as_slice())?;
    attempt.export(&planned)?;
    drop(planned);
    drop(files);
    Ok(attempt.finish())
}
