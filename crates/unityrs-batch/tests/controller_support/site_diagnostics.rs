//! Opt-in exact key stacks. Recording runs behind the hook's TRACING flag;
//! symbolization runs in the parent, outside the counted child altogether.
//! Linux diagnostics require binutils' addr2line, including inline symbols.
use std::{
    collections::BTreeSet,
    ffi::CStr,
    process::Command,
    sync::{Mutex, OnceLock},
};

type Frames = Vec<(String, usize)>;
static CENSUS: Mutex<Vec<(u64, Frames)>> = Mutex::new(Vec::new());

pub fn census_only(kind: &str, stdout: &str, stderr: &[u8], covered: &BTreeSet<u64>) {
    if !enabled() {
        return;
    }
    let stderr = String::from_utf8_lossy(stderr);
    *CENSUS.lock().unwrap() = stderr.lines().filter_map(parse).collect();
    for (key, _) in super::census(kind, stdout) {
        if !covered.contains(&key) {
            diagnose(key, &stderr);
        }
    }
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("UNITYRS_CONTROLLER_STACKS").is_some())
}

pub fn record(key: u64, frames: &[*mut libc::c_void]) {
    if !enabled() {
        return;
    }
    let frames: Frames = frames
        .iter()
        .map(|&address| {
            let mut info = std::mem::MaybeUninit::<libc::Dl_info>::zeroed();
            // SAFETY: dladdr writes one Dl_info into valid writable storage.
            assert_ne!(unsafe { libc::dladdr(address, info.as_mut_ptr()) }, 0);
            // SAFETY: dladdr succeeded and its object name is NUL terminated.
            let info = unsafe { info.assume_init() };
            let object = unsafe { CStr::from_ptr(info.dli_fname) }
                .to_str()
                .unwrap()
                .to_owned();
            // Resolve the call instruction, not the next inline function.
            (
                object,
                (address as usize).wrapping_sub(info.dli_fbase as usize + 1),
            )
        })
        .collect();
    eprintln!(
        "CONTROLLER_STACK key={key:016x} frames={}",
        serde_json::to_string(&frames).unwrap()
    );
}

fn parse(line: &str) -> Option<(u64, Frames)> {
    let rest = line.strip_prefix("CONTROLLER_STACK key=")?;
    let (key, frames) = rest.split_once(" frames=").unwrap();
    Some((
        u64::from_str_radix(key, 16).unwrap(),
        serde_json::from_str(frames).unwrap(),
    ))
}

pub fn diagnose(key: u64, stderr: &str) {
    if !enabled() {
        return;
    }
    let (_, frames) = stderr
        .lines()
        .filter_map(parse)
        .find(|(k, _)| *k == key)
        .unwrap();
    print_stack(key, &frames);
    // The closest census stack exposes exactly where a new scheduling path
    // diverged, without dumping every census allocation's stack.
    let census = CENSUS.lock().unwrap();
    if !census.iter().any(|(k, _)| *k == key)
        && let Some((near, baseline)) = census.iter().max_by_key(|(_, other)| {
            let pairs = || frames.iter().zip(other);
            (
                pairs().take_while(|(a, b)| a == b).count(),
                pairs().filter(|(a, b)| a == b).count(),
            )
        })
    {
        eprintln!("CONTROLLER_NEAREST_CENSUS new={key:016x} census={near:016x}");
        print_stack(*near, baseline);
    }
}

fn print_stack(key: u64, frames: &Frames) {
    eprintln!("CONTROLLER_KEY_STACK key={key:016x}");
    // Each image owns its offsets: resolving a libc offset against the test
    // executable would produce convincing but unrelated function names.
    let mut start = 0;
    while start < frames.len() {
        let object = &frames[start].0;
        let end = start
            + frames[start..]
                .iter()
                .take_while(|(o, _)| o == object)
                .count();
        let mut command = Command::new("addr2line");
        command.args(["-f", "-C", "-i", "-e", object]);
        command.args(
            frames[start..end]
                .iter()
                .map(|(_, offset)| format!("{offset:x}")),
        );
        let output = unityrs_testkit::child::output(&mut command).unwrap();
        assert!(output.status.success(), "addr2line failed");
        // addr2line emits function/source pairs. Never print source paths.
        for symbol in String::from_utf8(output.stdout).unwrap().lines().step_by(2) {
            eprintln!("  {symbol}");
        }
        start = end;
    }
    eprintln!("END_CONTROLLER_KEY_STACK");
}
