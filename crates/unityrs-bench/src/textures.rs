//! `textures-<format>.bundle` (plan section 2): a bundle per format, each a
//! `SerializedFile` of `Texture2D` objects with their data inline, mip 0
//! only, the bundle's data stored uncompressed (so a texture's decode is
//! measured, not its bundle's codec).
//!
//! At the full scale every texture is 1024 x 1024: 4 each of the raw
//! formats, 8 of each block format. At the gate scale, 2 of each at
//! 128 x 128.

use serde_json::{Map, Value, json};
use unityrs_testkit::Rng;
use unityrs_testkit::bundle::TestCodec;
use unityrs_testkit::ngui::texture;

use crate::astc::{self, Class};
use crate::blocks::{self, Kind};
use crate::{Fixture, Scale, UNITY_VERSION, bundle, seed_of, serialized};

/// One texture format of the set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// UnityPy's `TextureFormat` name.
    pub name: &'static str,
    /// Its value.
    pub id: i32,
    /// How its data is made.
    pub kind: Kind,
    /// How many textures at the full scale.
    pub full: usize,
}

const fn f(name: &'static str, id: i32, kind: Kind, full: usize) -> Format {
    Format {
        name,
        id,
        kind,
        full,
    }
}

/// The formats, in build order.
pub const FORMATS: [Format; 16] = [
    f("RGBA32", 4, Kind::Raw(4), 4),
    f("RGB24", 3, Kind::Raw(3), 4),
    f("ARGB4444", 2, Kind::Raw(2), 4),
    f("DXT1", 10, Kind::Seeded(8), 8),
    f("BC4", 26, Kind::Seeded(8), 8),
    f("ETC_RGB4", 34, Kind::Etc1, 8),
    f("EAC_R", 41, Kind::Seeded(8), 8),
    f("PVRTC_RGBA4", 33, Kind::Seeded(8), 8),
    f("DXT5", 12, Kind::Seeded(16), 8),
    f("BC5", 27, Kind::Seeded(16), 8),
    f("BC6H", 24, Kind::Bc6h, 8),
    f("BC7", 25, Kind::Bc7, 8),
    f("ETC2_RGBA8", 47, Kind::Seeded(16), 8),
    f("ATC_RGBA8", 36, Kind::Seeded(16), 8),
    f("ASTC_RGBA_4x4", 54, Kind::Astc(4, 4), 8),
    f("ASTC_RGBA_8x8", 57, Kind::Astc(8, 8), 8),
];

/// The format's file name.
#[must_use]
pub fn file_name(format: &Format) -> String {
    format!("textures-{}.bundle", format.name)
}

/// (count, side) at `scale`.
#[must_use]
pub fn dims(format: &Format, scale: Scale) -> (usize, usize) {
    scale.pick((format.full, 1024), (2, 128))
}

/// Texture `index`'s data of `format` at `side` x `side`.
pub fn data(format: &Format, index: usize, side: usize) -> Result<Vec<u8>, String> {
    let mut rng = Rng::new(seed_of(&format!("{}-{index}", format.name)));
    match format.kind {
        Kind::Astc(bw, bh) => astc::tile(bw, bh, format.kind.blocks(side, side)),
        kind => Ok(blocks::data(kind, side, side, &mut rng)),
    }
}

/// What the textures of `format` hold, block by block, as the manifest
/// records it: BC7's modes, ASTC's classes, and the count of each.
#[must_use]
pub fn mix(format: &Format, data: &[u8]) -> Value {
    let mut out = Map::new();
    let mut bump = |key: String| {
        let n = out.get(&key).and_then(Value::as_u64).unwrap_or(0);
        out.insert(key, json!(n + 1));
    };
    match format.kind {
        Kind::Bc7 => data.as_chunks::<16>().0.iter().for_each(|b| {
            bump(format!("mode:{}", blocks::bc7_mode(b).map_or(8, u32::from)));
        }),
        Kind::Astc(..) => data
            .as_chunks::<16>()
            .0
            .iter()
            .for_each(|b| bump(Class::of(b).name().to_owned())),
        _ => {}
    }
    Value::Object(out)
}

/// The bundle of `format` at `scale`.
pub fn build(format: &Format, scale: Scale) -> Result<Fixture, String> {
    let (count, side) = dims(format, scale);
    let edge = i32::try_from(side).unwrap_or(i32::MAX);
    let mut objects = Vec::with_capacity(count);
    let mut first_mix = Value::Null;
    for index in 0..count {
        let bytes = data(format, index, side)?;
        if index == 0 {
            first_mix = mix(format, &bytes);
        }
        let name = format!("{}_{index}", format.name);
        let path_id = i64::try_from(index + 1).unwrap_or(i64::MAX);
        objects.push(texture(
            path_id,
            name.as_bytes(),
            format.id,
            (edge, edge),
            bytes,
        ));
    }
    let file = serialized(22, UNITY_VERSION, &objects).map_err(|e| e.to_string())?;
    let cab = format!("CAB-bench-textures-{}", format.name.to_ascii_lowercase());
    let bytes = bundle(
        UNITY_VERSION,
        vec![(cab, file, 4)],
        TestCodec::None,
        128 << 10,
    );
    let inventory = json!({
        "kind": "textures",
        "format": format.name,
        "format_id": format.id,
        "textures": count,
        "width": side,
        "height": side,
        "pixels": count * side * side,
        "data_bytes": count * format.kind.blocks(side, side) * format.kind.block_bytes(),
        "block_mix_each": first_mix,
    });
    Ok(Fixture {
        name: file_name(format),
        bytes,
        inventory,
    })
}
