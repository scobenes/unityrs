use std::{collections::BTreeMap, io, sync::Mutex};
use unityrs_batch::start::host::{self, Class, Host, Limit, Requirements, Resource};

struct Facts {
    files: BTreeMap<String, String>,
    denied: Option<String>,
    finite: Option<(Resource, Limit)>,
    privileged: bool,
    cap_error: bool,
    calls: Mutex<Vec<String>>,
}
impl Facts {
    fn new() -> Self {
        let mut f = Self {
            files: BTreeMap::new(),
            denied: None,
            finite: None,
            privileged: false,
            cap_error: false,
            calls: Mutex::new(Vec::new()),
        };
        for (path, text) in [
            ("/proc/sys/vm/overcommit_memory", "0"),
            ("/proc/sys/vm/max_map_count", "65530"),
            ("/proc/meminfo", "MemTotal: 1048576 kB"),
            ("/proc/self/uid_map", "0 0 4294967295"),
            ("/proc/self/cgroup", "0::/parent/job"),
            (
                "/proc/self/mountinfo",
                "1 2 0:3 / /cg rw - cgroup2 cgroup rw",
            ),
            ("/cg/parent/job/memory.max", "max"),
            ("/cg/parent/memory.max", "max"),
            ("/cg/parent/job/pids.max", "max"),
            ("/cg/parent/pids.max", "max"),
        ] {
            f.set(path, text);
        }
        f
    }
    fn set(&mut self, path: &str, text: &str) {
        self.files.insert(path.into(), text.into());
    }
}
impl Host for Facts {
    fn lines(&self, path: &str, visit: &mut dyn FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        self.calls.lock().unwrap().push(path.into());
        if self.denied.as_deref() == Some(path) {
            return Err(io::Error::from_raw_os_error(libc::EACCES));
        }
        let text = self
            .files
            .get(path)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        for line in text.lines() {
            visit(line)?;
        }
        Ok(())
    }
    fn limit(&self, r: Resource) -> io::Result<Limit> {
        self.calls.lock().unwrap().push(r.name().into());
        Ok(self.finite.filter(|(kind, _)| *kind == r).map_or(
            Limit {
                soft: None,
                hard: None,
            },
            |(_, l)| l,
        ))
    }
    fn raise(&self, r: Resource, _: Limit) -> io::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("raise {}", r.name()));
        Ok(())
    }
    fn privileged(&self) -> io::Result<bool> {
        if self.cap_error {
            Err(io::Error::from_raw_os_error(libc::EPERM))
        } else {
            Ok(self.privileged)
        }
    }
}
fn needs() -> Requirements {
    Requirements {
        bound: 1024 * 1024,
        address: 4 * 1024 * 1024,
        maps: 8_704,
        files: 92,
        threads: 10,
    }
}
#[test]
fn ordered_checks_accept_equal_limits_and_raise_to_the_hard_limits() {
    let facts = Facts::new();
    host::check(&facts, needs()).unwrap();
    let calls = facts.calls.lock().unwrap();
    assert_eq!(
        &calls[..5],
        [
            "RLIMIT_AS",
            "RLIMIT_DATA",
            "/proc/sys/vm/overcommit_memory",
            "/proc/sys/vm/max_map_count",
            "/proc/meminfo"
        ]
    );
    assert!(
        calls
            .iter()
            .position(|s| s == "raise RLIMIT_NOFILE")
            .unwrap()
            < calls
                .iter()
                .position(|s| s == "raise RLIMIT_FSIZE")
                .unwrap()
    );
    drop(calls);
    for (r, needed) in [
        (Resource::AddressSpace, needs().address),
        (Resource::Data, needs().address),
        (Resource::OpenFiles, needs().files),
        (Resource::Processes, needs().threads),
    ] {
        let mut facts = Facts::new();
        facts.finite = Some((
            r,
            Limit {
                soft: Some(needed),
                hard: Some(needed),
            },
        ));
        host::check(&facts, needs()).unwrap();
        facts.finite = Some((
            r,
            Limit {
                soft: Some(needed - 1),
                hard: Some(needed - 1),
            },
        ));
        let error = host::check(&facts, needs()).unwrap_err();
        assert_eq!(error.class, Class::RlimitTooLow);
        assert_eq!(error.subject, r.name());
        assert_eq!(error.needed, Some(needed));
        assert_eq!(error.found, Some(needed - 1));
    }
    let mut facts = Facts::new();
    facts.finite = Some((
        Resource::FileSize,
        Limit {
            soft: Some(0),
            hard: Some(u64::MAX - 1),
        },
    ));
    let error = host::check(&facts, needs()).unwrap_err();
    assert_eq!(error.class, Class::RlimitTooLow);
    assert_eq!(error.needed, None);
}
#[test]
fn refuses_overcommit_maps_memtotal_and_every_unreadable_proc_file() {
    for (path, text, class) in [
        (
            "/proc/sys/vm/overcommit_memory",
            "2",
            Class::OvercommitStrict,
        ),
        ("/proc/sys/vm/max_map_count", "8192", Class::MapCountTooLow),
        ("/proc/meminfo", "MemTotal: 1 kB", Class::MemoryTooSmall),
        (
            "/proc/meminfo",
            "MemTotal: nonsense",
            Class::ProcfsUnreadable,
        ),
    ] {
        let mut f = Facts::new();
        f.set(path, text);
        assert_eq!(host::check(&f, needs()).unwrap_err().class, class);
    }
    for path in [
        "/proc/sys/vm/overcommit_memory",
        "/proc/sys/vm/max_map_count",
        "/proc/meminfo",
        "/proc/self/cgroup",
        "/proc/self/mountinfo",
        "/proc/self/uid_map",
    ] {
        let mut f = Facts::new();
        f.denied = Some(path.into());
        let e = host::check(&f, needs()).unwrap_err();
        assert_eq!(e.class, Class::ProcfsUnreadable);
        assert_eq!(e.subject, path);
    }
    let mut f = Facts::new();
    f.cap_error = true;
    assert_eq!(host::check(&f, needs()).unwrap_err().subject, "capget");
}
#[test]
fn cgroup_v2_walks_ancestors_and_skips_absent_levels_and_max() {
    let mut f = Facts::new();
    f.set("/cg/parent/memory.max", "1023");
    let e = host::check(&f, needs()).unwrap_err();
    assert_eq!(e.class, Class::MemoryTooSmall);
    assert_eq!(e.subject, "cgroup");
    assert_eq!(e.found, Some(1023));
    f.files.remove("/cg/parent/memory.max");
    f.set("/cg/memory.max", "1023");
    assert_eq!(
        host::check(&f, needs()).unwrap_err().class,
        Class::MemoryTooSmall
    );
    f.set("/cg/memory.max", "max");
    host::check(&f, needs()).unwrap();
    f.denied = Some("/cg/parent/memory.max".into());
    assert_eq!(
        host::check(&f, needs()).unwrap_err().class,
        Class::CgroupUnreadable
    );
    f.denied = None;
    f.set("/cg/parent/pids.max", "10");
    f.set("/cg/parent/pids.current", "2");
    let e = host::check(&f, needs()).unwrap_err();
    assert_eq!(e.class, Class::RlimitTooLow);
    assert_eq!(e.subject, "/parent");
    assert_eq!(e.needed, Some(9));
    assert_eq!(e.found, Some(8));
    f.set("/cg/parent/pids.current", "1");
    host::check(&f, needs()).unwrap();
    f.denied = Some("/cg/parent/pids.current".into());
    assert_eq!(
        host::check(&f, needs()).unwrap_err().class,
        Class::CgroupUnreadable
    );
}
#[test]
fn hybrid_prefers_v1_memory_and_longest_mount_root() {
    let mut f = Facts::new();
    f.set(
        "/proc/self/cgroup",
        "4:memory:/container/parent/job\n0::/parent/job",
    );
    f.set("/proc/self/mountinfo","1 2 0:3 / /cg rw - cgroup2 cgroup rw\n2 3 0:4 / /wrong rw - cgroup cgroup rw,memory\n3 4 0:4 /container /visible\\040memory rw - cgroup cgroup rw,memory");
    f.set(
        "/visible memory/parent/job/memory.stat",
        "hierarchical_memory_limit 1023",
    );
    let e = host::check(&f, needs()).unwrap_err();
    assert_eq!(e.class, Class::MemoryTooSmall);
    assert_eq!(e.found, Some(1023));
    f.files.remove("/visible memory/parent/job/memory.stat");
    host::check(&f, needs()).unwrap();
    f.denied = Some("/visible memory/parent/job/memory.stat".into());
    assert_eq!(
        host::check(&f, needs()).unwrap_err().class,
        Class::CgroupUnreadable
    );
}
#[test]
fn invisible_hierarchy_and_namespace_privilege_rules_are_explicit() {
    let mut f = Facts::new();
    f.set(
        "/proc/self/mountinfo",
        "1 2 0:3 /unrelated /cg rw - cgroup2 cgroup rw",
    );
    f.set("/cg/parent/memory.max", "1");
    host::check(&f, needs()).unwrap();
    f.finite = Some((
        Resource::Processes,
        Limit {
            soft: Some(1),
            hard: None,
        },
    ));
    f.privileged = true;
    host::check(&f, needs()).unwrap();
    f.set("/proc/self/uid_map", "0 1000 1");
    assert_eq!(
        host::check(&f, needs()).unwrap_err().class,
        Class::RlimitTooLow
    );
    f.set("/proc/self/cgroup", "");
    f.finite = None;
    host::check(&f, needs()).unwrap();
}

#[test]
fn hostile_mountinfo_octal_escapes_are_typed_unreadable_refusals() {
    for escape in [r"\377", r"\400", r"\440", r"\3"] {
        let mut facts = Facts::new();
        facts.set(
            "/proc/self/mountinfo",
            &format!("1 2 0:3 / /cg{escape} rw - cgroup2 cgroup rw"),
        );
        let error = host::check(&facts, needs()).unwrap_err();
        assert_eq!(error.class, Class::ProcfsUnreadable, "{escape}");
        assert_eq!(error.subject, "/proc/self/mountinfo", "{escape}");
    }
}
