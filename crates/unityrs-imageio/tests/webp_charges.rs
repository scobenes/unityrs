//! CR10: measure the complete heap and sweep every fallible encoder allocation.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;
use unityrs_imageio::{
    Buffer, Charger, Image, Mode, WebpError, WebpOptions, encode_webp, webp_plan,
};
use unityrs_testkit::counted::{
    Allocator, Tally, count_this_thread, counted, stop_counting_this_thread,
};
static LIVE: AtomicUsize = AtomicUsize::new(0);
static BASE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static HELD: AtomicUsize = AtomicUsize::new(0);
static OVER: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static FAIL: AtomicUsize = AtomicUsize::new(0);
static MEASURE: Mutex<()> = Mutex::new(());
struct Measurement {
    _lock: MutexGuard<'static, ()>,
}
impl Drop for Measurement {
    fn drop(&mut self) {
        // libtest frees thread bookkeeping after returning from the test;
        // those frees must not enter the next test's measurement window.
        stop_counting_this_thread();
    }
}
fn measure() -> Measurement {
    let lock = MEASURE.lock().unwrap();
    LIVE.store(0, Relaxed);
    BASE.store(0, Relaxed);
    HELD.store(0, Relaxed);
    FAIL.store(0, Relaxed);
    count_this_thread();
    Measurement { _lock: lock }
}
struct Watch;
impl Tally for Watch {
    fn refuses(&self, _: usize) -> bool {
        counted() && CALLS.fetch_add(1, Relaxed) + 1 == FAIL.load(Relaxed)
    }
    fn grew(&self, n: usize, _: bool) {
        let above = (LIVE.fetch_add(n, Relaxed) + n).saturating_sub(BASE.load(Relaxed));
        PEAK.fetch_max(above, Relaxed);
        OVER.fetch_max(above.saturating_sub(HELD.load(Relaxed)), Relaxed);
    }
    fn shrank(&self, n: usize) {
        LIVE.fetch_sub(n, Relaxed);
    }
}
#[global_allocator]
static ALLOC: Allocator<Watch> = Allocator(Watch);
#[derive(Debug)]
struct Held(usize);
impl Drop for Held {
    fn drop(&mut self) {
        let rest = HELD.fetch_sub(self.0, Relaxed) - self.0;
        OVER.fetch_max(
            LIVE.load(Relaxed)
                .saturating_sub(BASE.load(Relaxed))
                .saturating_sub(rest),
            Relaxed,
        );
    }
}
struct Grant(Option<Buffer>);
impl Charger for Grant {
    type Charge = Held;
    type Refusal = ();
    fn charge(&mut self, b: Buffer, requests: &[usize]) -> Result<Held, ()> {
        if self.0 == Some(b) {
            return Err(());
        }
        let n = requests.iter().sum();
        HELD.fetch_add(n, Relaxed);
        Ok(Held(n))
    }
}
fn start() {
    BASE.store(LIVE.load(Relaxed), Relaxed);
    CALLS.store(0, Relaxed);
    PEAK.store(0, Relaxed);
    OVER.store(0, Relaxed);
}
fn settled() {
    assert_eq!(LIVE.load(Relaxed), BASE.load(Relaxed), "heap leaked");
    assert_eq!(HELD.load(Relaxed), 0, "charge leaked");
    assert_eq!(
        OVER.load(Relaxed),
        0,
        "allocation outlived or exceeded charge"
    );
}
#[test]
fn webp_allocations_are_charged_fallible_and_freed() {
    let _measurement = measure();
    for (width, height) in [
        (1, 1),
        (15, 17),
        (257, 129),
        (1024, 1024),
        (16383, 1),
        (1, 16383),
    ] {
        for mode in Mode::EXTRACTOR {
            let mut state = 0x1234_5678_u32;
            let raw: Vec<u8> = (0..width * height * mode.channels() as u32)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (state >> 24) as u8
                })
                .collect();
            let image = Image::new(width, height, mode, &raw).unwrap();
            let options = WebpOptions::default();
            start();
            let file = encode_webp(&image, options, &mut Grant(None)).unwrap();
            let calls = CALLS.load(Relaxed);
            let peak = PEAK.load(Relaxed);
            drop(file);
            settled();
            eprintln!("WebP {width}x{height} {mode}: peak={peak} allocations={calls}");
            for b in [Buffer::WebpState, Buffer::Output] {
                start();
                let e = encode_webp(&image, options, &mut Grant(Some(b))).unwrap_err();
                assert!(matches!(e, WebpError::Refused { buffer, .. } if buffer == b));
                assert_eq!(CALLS.load(Relaxed), 0);
                settled();
            }
        }
    }
}

fn pixels(mode: Mode, width: u32, height: u32, kind: &str) -> Vec<u8> {
    let mut state = 0x1234_5678_u32;
    (0..width * height)
        .flat_map(|i| (0..mode.channels()).map(move |c| (i, c)))
        .map(|(i, c)| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            match kind {
                "gradient" => (i % width * 7 + i / width * 13 + c as u32 * 53) as u8,
                "alpha" if c == mode.channels() - 1 => [0, 1, 127, 254, 255][i as usize % 5],
                "palette" => ((i % width + i / width * 3) % 16 * 13 + c as u32 * 17) as u8,
                _ => (state >> 24) as u8,
            }
        })
        .collect()
}

fn refuse(image: &Image<'_>, options: WebpOptions, fail: usize, lossless: bool) {
    start();
    FAIL.store(fail, Relaxed);
    let result = encode_case(image, options, lossless);
    FAIL.store(0, Relaxed);
    assert!(CALLS.load(Relaxed) >= fail, "refusal was not reached");
    assert!(
        matches!(result, Err(WebpError::Memory)),
        "allocation {fail}: {result:?}"
    );
    drop(result);
    settled();
}

fn encode_case(
    image: &Image<'_>,
    options: WebpOptions,
    lossless: bool,
) -> Result<unityrs_imageio::Webp<Held>, WebpError<()>> {
    #[cfg(feature = "test-hooks")]
    if lossless {
        return unityrs_imageio::test_encode_webp_lossless(image, options, &mut Grant(None));
    }
    assert!(!lossless, "lossless sweep requires test-hooks");
    encode_webp(image, options, &mut Grant(None))
}

fn sweep(extended: bool) {
    let _measurement = measure();
    let began = Instant::now();
    let mut cases = 0;
    let mut refusals = 0;
    // The gradient tries multiple alpha filters; noise reaches the palette
    // co-occurrence matrix in method 6; endpoints cover transparent RGB cleanup.
    for (mode, width, height, kind) in [
        (Mode::Rgba, 31, 33, "gradient"),
        (Mode::Rgba, 17, 17, "random"),
        (Mode::Rgba, 15, 17, "alpha"),
        (Mode::Rgb, 17, 17, "palette"),
        (Mode::La, 15, 17, "gradient"),
        (Mode::L, 15, 17, "random"),
    ] {
        let raw = pixels(mode, width, height, kind);
        let image = Image::new(width, height, mode, &raw).unwrap();
        for method in 0..=6 {
            for &(quality, exact) in if extended {
                &[(0.0, false), (95.0, false), (100.0, true)][..]
            } else {
                &[(80.0, false)][..]
            } {
                let options = WebpOptions {
                    method,
                    quality,
                    exact,
                };
                for lossless in [false, true] {
                    if lossless && !cfg!(feature = "test-hooks") {
                        continue;
                    }
                    start();
                    let file = encode_case(&image, options, lossless).unwrap();
                    let calls = CALLS.load(Relaxed);
                    drop(file);
                    settled();
                    for fail in 1..=calls {
                        refuse(&image, options, fail, lossless);
                    }
                    cases += 1;
                    refusals += calls;
                    eprintln!(
                        "swept {mode} {width}x{height} {kind} method={method} quality={quality} exact={exact} lossless={lossless}: {calls} refusals"
                    );
                }
            }
        }
    }
    eprintln!(
        "WebP sweep: {cases} cases, {refusals} refusals, {:?}",
        began.elapsed()
    );
}

#[test]
fn every_allocation_refusal_is_clean() {
    sweep(false);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run with --profile sweep --features test-hooks"
)]
fn every_allocation_refusal_at_quality_boundaries_is_clean() {
    sweep(true);
}

#[test]
fn alpha_trial_early_failure_has_an_initialized_bitwriter() {
    let _measurement = measure();
    let raw = pixels(Mode::Rgba, 31, 33, "gradient");
    let image = Image::new(31, 33, Mode::Rgba, &raw).unwrap();
    refuse(&image, WebpOptions::default(), 8, false);
}

#[test]
fn palette_cooccurrence_failure_frees_the_matrix() {
    let _measurement = measure();
    let raw = pixels(Mode::Rgba, 17, 17, "random");
    let image = Image::new(17, 17, Mode::Rgba, &raw).unwrap();
    refuse(
        &image,
        WebpOptions {
            method: 6,
            ..WebpOptions::default()
        },
        180,
        false,
    );
}
#[test]
fn webp_replays_dimension_refusals_and_rejects_invalid_options() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/webp.json")).unwrap();
    for row in fixture["refusals"].as_array().unwrap() {
        assert_eq!(row["error"], "ValueError");
        assert_eq!(row["message"], "encoding error 5");
        let (width, height) = (
            row["width"].as_u64().unwrap() as u32,
            row["height"].as_u64().unwrap() as u32,
        );
        assert!(matches!(
            webp_plan(width, height),
            Err(WebpError::Dimensions { .. })
        ));
        let raw = vec![0; width as usize * height as usize * 4];
        let image = Image::new(width, height, Mode::Rgba, &raw).unwrap();
        assert!(matches!(
            encode_webp(
                &image,
                WebpOptions::default(),
                &mut Grant(Some(Buffer::WebpState))
            ),
            Err(WebpError::Dimensions { .. })
        ));
    }
    let image = Image::new(1, 1, Mode::Rgb, &[0; 3]).unwrap();
    for quality in [f32::NAN, f32::INFINITY, -1.0, 100.1] {
        assert!(matches!(
            encode_webp(
                &image,
                WebpOptions {
                    quality,
                    ..WebpOptions::default()
                },
                &mut Grant(Some(Buffer::WebpState))
            ),
            Err(WebpError::Options)
        ));
    }
    assert!(matches!(
        encode_webp(
            &image,
            WebpOptions {
                method: 7,
                ..WebpOptions::default()
            },
            &mut Grant(Some(Buffer::WebpState))
        ),
        Err(WebpError::Options)
    ));
}

#[test]
fn pillow_validates_options_before_picture_dimensions() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/webp.json")).unwrap();
    for row in fixture["options_refusals"].as_array().unwrap() {
        assert_eq!(row["error"], "ValueError");
        assert_eq!(row["message"], "invalid configuration");
        let width = u32::try_from(row["width"].as_u64().unwrap()).unwrap();
        let height = u32::try_from(row["height"].as_u64().unwrap()).unwrap();
        let pixels = vec![0; width as usize * height as usize * 4];
        let image = Image::new(width, height, Mode::Rgba, &pixels).unwrap();
        let options = WebpOptions {
            quality: row["quality"].as_f64().unwrap() as f32,
            method: u8::try_from(row["method"].as_u64().unwrap()).unwrap(),
            exact: false,
        };
        assert!(matches!(
            encode_webp(&image, options, &mut Grant(Some(Buffer::WebpState))),
            Err(WebpError::Options)
        ));
    }
}

#[test]
fn higher_methods_plan_their_larger_alpha_workspace() {
    use unityrs_imageio::{Unmetered, webp_plan_with_options};
    for (width, height) in [(1, 16383), (16383, 2)] {
        let mut state = 0x1234_5678_u32;
        let pixels: Vec<u8> = (0..width as usize * height as usize * 4)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        let image = Image::new(width, height, Mode::Rgba, &pixels).unwrap();
        for method in [5, 6] {
            let options = WebpOptions {
                method,
                quality: 95.0,
                exact: true,
            };
            let plan = webp_plan_with_options(width, height, options).unwrap();
            assert_eq!(
                plan.workspace,
                (32 << 20) + width as usize * height as usize * 256
            );
            assert!(encode_webp(&image, options, &mut Unmetered).is_ok());
        }
    }
}

#[test]
fn palette_images_are_refused_before_any_charge() {
    let palette = [7u8; 768];
    let pixels = [0u8; 6];
    let image = Image::with_palette(3, 2, &pixels, &palette).unwrap();
    let _measurement = measure();
    for options in [
        WebpOptions::default(),
        WebpOptions {
            quality: f32::NAN,
            ..WebpOptions::default()
        },
    ] {
        assert!(matches!(
            encode_webp(&image, options, &mut Grant(None)),
            Err(WebpError::Palette)
        ));
    }
    settled();
}
