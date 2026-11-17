// Ported from UnityPy 1.25.2 (MIT): export/SpriteHelper.py:130-301. See THIRD_PARTY_LICENSES.
//! Standalone adapters reuse the renderer's mask, mesh and triangle operations.
use super::{Image, Sheet, SpriteError, canvas, exact::Int, render};
use std::sync::Arc;
use unityrs_core::limits::Meter;
use unityrs_meshdecode::{Triangles, VertexArray};

/// A polygon mask over an image in decoded (bottom-up) storage order.
pub fn mask_image(
    image: &Image,
    positions: Option<&VertexArray>,
    triangles: Triangles,
    pixels_to_units: f64,
    meter: &Arc<Meter>,
) -> Result<Image, SpriteError> {
    let sheet = Sheet::new(image, None, meter)?;
    let crop = canvas::crop(
        &sheet,
        [0, 0, side(image.width())?, side(image.height())?],
        meter,
    )?;
    render::mask_parts(&crop, positions, || Ok(triangles), pixels_to_units, meter)
        .map(canvas::Canvas::into_image)
}

/// Renders the first submesh, preserving the standalone helper's top-down output.
pub fn render_mesh(
    image: &Image,
    positions: Option<&VertexArray>,
    uv: Option<&VertexArray>,
    triangles: Triangles,
    pixels_to_units: f64,
    meter: &Arc<Meter>,
) -> Result<Image, SpriteError> {
    let sheet = Sheet::new(image, None, meter)?;
    render::mesh_parts(
        &sheet,
        positions,
        uv,
        || Ok(triangles),
        pixels_to_units,
        meter,
    )
    .map(canvas::Canvas::into_image)
}

/// Copies one triangle into a decoded destination, returning top-down pixels.
pub fn copy_image_triangle(
    source: &Image,
    source_triangle: [[i64; 2]; 3],
    destination: &Image,
    destination_triangle: [[f64; 2]; 3],
    meter: &Arc<Meter>,
) -> Result<Image, SpriteError> {
    let sheet = Sheet::new(source, None, meter)?;
    let dst = Sheet::new(destination, None, meter)?;
    let mut out = canvas::crop(
        &dst,
        [
            0,
            0,
            side(destination.width())?,
            side(destination.height())?,
        ],
        meter,
    )?;
    render::copy_triangle(
        &sheet,
        source_triangle.map(|p| p.map(|v| Int::I(i128::from(v)))),
        &mut out,
        destination_triangle,
        meter,
    )?;
    Ok(out.into_image())
}

fn side(value: u32) -> Result<i32, SpriteError> {
    i32::try_from(value).map_err(|_| SpriteError::TooLarge { what: "canvas" })
}
