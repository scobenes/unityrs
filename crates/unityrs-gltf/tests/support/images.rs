use super::MINIMAL_JPEG;
use unityrs_gltf::Mime;
pub type Case = (&'static str, Mime, Vec<u8>, Option<&'static str>);

pub fn review_cases() -> Vec<Case> {
    let png = include_bytes!("../goldens/pixel.png");
    let mut no_idat = png[..33].to_vec();
    no_idat.extend_from_slice(&png[png.len() - 12..]);
    assert_eq!(no_idat.len(), 45);
    let mut bad_sof_length = MINIMAL_JPEG.to_vec();
    bad_sof_length[5] = 1;
    let mut truncated_sof = MINIMAL_JPEG.to_vec();
    truncated_sof[5] = 30;
    let mut bad_png_length = png.to_vec();
    bad_png_length[11] = 12;
    let mut framed_sof = vec![0xff, 0xd8, 0xff, 0xe0, 0, 8, 0xff, 0xc0, 0, 2, 0, 0];
    framed_sof.extend_from_slice(&[0xff, 0xd9]);
    let mut standalone = vec![0xff, 0xd8, 0xff, 0x01, 0xff, 0xd0, 0xff, 0xff];
    standalone.extend_from_slice(&MINIMAL_JPEG[2..]);
    let mut dhp = MINIMAL_JPEG.to_vec();
    dhp[3] = 0xde;
    let mut bad_channels = MINIMAL_JPEG.to_vec();
    bad_channels[11] = 4;
    let mut crc = png.to_vec();
    crc[29..33].fill(0);
    let mut trailing_png = png.to_vec();
    trailing_png.extend_from_slice(b"trailing");
    let mut trailing_jpeg = MINIMAL_JPEG.to_vec();
    trailing_jpeg.extend_from_slice(b"trailing");
    let mut cases = vec![
        (
            "png-without-idat",
            Mime::Png,
            no_idat,
            Some("IMAGE_UNEXPECTED_EOS"),
        ),
        (
            "jpeg-marker-only",
            Mime::Jpeg,
            vec![0xff, 0xd8, 0xff, 0xc0, 0xff, 0xd9],
            Some("IMAGE_UNRECOGNIZED_FORMAT"),
        ),
        (
            "jpeg-bad-sof-length",
            Mime::Jpeg,
            bad_sof_length,
            Some("IMAGE_DATA_INVALID"),
        ),
        (
            "jpeg-truncated-sof",
            Mime::Jpeg,
            truncated_sof,
            Some("IMAGE_UNEXPECTED_EOS"),
        ),
        (
            "jpeg-sof-in-payload",
            Mime::Jpeg,
            framed_sof,
            Some("IMAGE_UNEXPECTED_EOS"),
        ),
        (
            "jpeg-bad-channels",
            Mime::Jpeg,
            bad_channels,
            Some("IMAGE_DATA_INVALID"),
        ),
        (
            "png-bad-ihdr-length",
            Mime::Png,
            bad_png_length,
            Some("IMAGE_DATA_INVALID"),
        ),
        ("minimal-png", Mime::Png, png.to_vec(), None),
        ("minimal-jpeg", Mime::Jpeg, MINIMAL_JPEG.to_vec(), None),
        ("png-idat-header-only", Mime::Png, png[..41].to_vec(), None),
        (
            "jpeg-sof-only",
            Mime::Jpeg,
            MINIMAL_JPEG[..15].to_vec(),
            None,
        ),
        ("png-ignored-crc", Mime::Png, crc, None),
        ("png-trailing", Mime::Png, trailing_png, None),
        ("jpeg-trailing", Mime::Jpeg, trailing_jpeg, None),
        ("jpeg-standalone-markers", Mime::Jpeg, standalone, None),
        ("jpeg-dhp", Mime::Jpeg, dhp, None),
    ];
    cases.extend(png_metadata_cases());
    for (name, kind) in [
        ("png-bad-chrm-length", b"cHRM"),
        ("png-bad-srgb-length", b"sRGB"),
        ("png-bad-phys-length", b"pHYs"),
        ("png-bad-gama-length", b"gAMA"),
    ] {
        cases.push((
            name,
            Mime::Png,
            with_chunk(kind, &[], false),
            Some("IMAGE_DATA_INVALID"),
        ));
    }

    cases
}

fn with_chunk(kind: &[u8], data: &[u8], before_ihdr: bool) -> Vec<u8> {
    let png = include_bytes!("../goldens/pixel.png");
    let at = if before_ihdr { 8 } else { 33 };
    let mut out = png[..at].to_vec();
    out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&[0; 4]); // The oracle does not verify CRC values.
    out.extend_from_slice(&png[at..]);
    out
}
fn png_metadata_cases() -> Vec<Case> {
    let png = include_bytes!("../goldens/pixel.png");
    let mut huge_chunk = with_chunk(b"junk", &[], false);
    huge_chunk[33..37].fill(0xff);
    let mut huge_idat = png.to_vec();
    huge_idat[33..37].fill(0xff);
    let mut header_fields = png.to_vec();
    header_fields[16..29].fill(0); // Dimensions/bit depth/color type are metadata.
    let custom_gamma = with_chunk(b"gAMA", &[0, 0, 0, 1], false);
    let mut overridden_gamma = custom_gamma[..49].to_vec();
    overridden_gamma.extend_from_slice(&with_chunk(b"sRGB", &[0], false)[33..]);
    let srgb: Vec<u8> = [
        31_270u32, 32_900, 64_000, 33_000, 30_000, 60_000, 15_000, 6_000,
    ]
    .into_iter()
    .flat_map(u32::to_be_bytes)
    .collect();
    let cases = vec![
        (
            "png-large-chunk-length",
            Mime::Png,
            huge_chunk,
            Some("IMAGE_UNEXPECTED_EOS"),
        ),
        ("png-large-idat-length", Mime::Png, huge_idat, None),
        ("png-header-fields", Mime::Png, header_fields, None),
        (
            "png-chunk-before-ihdr",
            Mime::Png,
            with_chunk(b"junk", &[], true),
            None,
        ),
        (
            "png-custom-gamma",
            Mime::Png,
            custom_gamma,
            Some("IMAGE_FEATURES_UNSUPPORTED"),
        ),
        (
            "png-srgb-overrides-gamma",
            Mime::Png,
            overridden_gamma,
            None,
        ),
        (
            "png-srgb-gamma",
            Mime::Png,
            with_chunk(b"gAMA", &45_455u32.to_be_bytes(), false),
            None,
        ),
        (
            "png-linear-gamma",
            Mime::Png,
            with_chunk(b"gAMA", &100_000u32.to_be_bytes(), false),
            None,
        ),
        (
            "png-srgb-primaries",
            Mime::Png,
            with_chunk(b"cHRM", &srgb, false),
            None,
        ),
        (
            "png-custom-primaries",
            Mime::Png,
            with_chunk(b"cHRM", &[0; 32], false),
            Some("IMAGE_FEATURES_UNSUPPORTED"),
        ),
        (
            "png-icc-profile",
            Mime::Png,
            with_chunk(b"iCCP", &[], false),
            Some("IMAGE_FEATURES_UNSUPPORTED"),
        ),
        (
            "png-square-pixels",
            Mime::Png,
            with_chunk(b"pHYs", &[0; 9], false),
            None,
        ),
        (
            "png-nonsquare-pixels",
            Mime::Png,
            with_chunk(b"pHYs", &[0, 0, 0, 1, 0, 0, 0, 2, 0], false),
            Some("IMAGE_FEATURES_UNSUPPORTED"),
        ),
    ];
    cases
}
