//! The records' canonical lines (`docs/CANONICAL_DUMP.md` section 4): the
//! writers, which [`super`] re-exports.

use unityrs_core::RawStr;
use unityrs_core::typetree::NodeList;
use unityrs_core::version::FlagsClass;

use super::{
    BlockRow, BundleL1, DirRow, FORMAT, FORMAT_VERSION, FileErrorKind, FileL1, FileRecord,
    ObjectRecord, Resave, SerializedL1, TypeRecord, WebL1, file_type_name, parsed_as_name,
};
use crate::Levels;
use crate::json::{Obj, write_array, write_hex_str, write_name, write_packed, write_str};

/// The `dump` header (section 4.1). `producer` is the producer object's
/// members, already canonical JSON text, in order (`"name":...` first).
#[must_use]
pub fn header(levels: Levels, producer_members: &str) -> String {
    let mut out = String::new();
    let mut o = Obj::new(&mut out);
    o.str("rec", "dump");
    o.str("format", FORMAT);
    o.int("version", FORMAT_VERSION);
    let lv = o.key("levels");
    write_array(lv, &levels.names(), |out, n| write_str(out, n));
    let p = o.key("producer");
    p.push('{');
    p.push_str(producer_members);
    p.push('}');
    o.end();
    out
}

/// The `end` record (section 4.4).
#[must_use]
pub fn end(files: u64, objects: u64) -> String {
    let mut out = String::new();
    let mut o = Obj::new(&mut out);
    o.str("rec", "end");
    o.int("files", files);
    o.int("objects", objects);
    o.end();
    out
}

fn write_identity(out: &mut String, file: &[RawStr]) {
    write_array(out, file, |out, n| write_name(out, n.as_bytes()));
}

/// A `file` record's line.
#[must_use]
pub fn file_line(rec: &FileRecord) -> String {
    let mut out = String::new();
    let mut o = Obj::new(&mut out);
    o.str("rec", "file");
    write_identity(o.key("file"), &rec.file);
    {
        let l0 = o.key("l0");
        let mut m = Obj::new(l0);
        m.str("type", file_type_name(rec.l0.ty));
        m.str("parsed_as", parsed_as_name(rec.l0.parsed_as));
        m.int("size", rec.l0.size);
        m.hex("sha256", Some(&rec.l0.sha256));
        m.end();
    }
    match &rec.l1 {
        None => o.null("l1"),
        Some(FileL1::Bundle(b)) => bundle_l1(o.key("l1"), b),
        Some(FileL1::Web(w)) => web_l1(o.key("l1"), w),
        Some(FileL1::Serialized(s)) => serialized_l1(o.key("l1"), s),
    }
    match &rec.error {
        None => o.null("error"),
        Some(e) => {
            let mut m = Obj::new(o.key("error"));
            m.str(
                "kind",
                match e.kind {
                    FileErrorKind::Detect => "detect",
                    FileErrorKind::Parse => "parse",
                    FileErrorKind::Crash => "crash",
                },
            );
            if let Some(n) = &e.note {
                m.str("note", n);
            }
            m.end();
        }
    }
    o.end();
    out
}

fn block_row(out: &mut String, b: &BlockRow) {
    use std::fmt::Write as _;
    let _ = write!(out, "[{},{},{}]", b.uncompressed, b.compressed, b.flags);
}

fn dir_rows(out: &mut String, rows: &[DirRow]) {
    use std::fmt::Write as _;
    write_array(out, rows, |out, r| {
        let _ = write!(out, "[{},{},", r.offset, r.size);
        match r.flags {
            Some(f) => {
                let _ = write!(out, "{f}");
            }
            None => out.push_str("null"),
        }
        out.push(',');
        write_name(out, r.path.as_bytes());
        out.push(']');
    });
}

fn bundle_l1(out: &mut String, b: &BundleL1) {
    let mut o = Obj::new(out);
    o.str("kind", "bundle");
    o.name("signature", Some(b.signature.as_bytes()));
    o.int("format", b.format);
    o.name("version_player", Some(b.version_player.as_bytes()));
    o.name("version_engine", Some(b.version_engine.as_bytes()));
    match &b.engine_version_packed {
        Some(p) => write_packed(o.key("engine_version_packed"), p),
        None => o.null("engine_version_packed"),
    }
    o.opt_int("flags", b.flags);
    match b.flags_class {
        Some(FlagsClass::New) => o.str("flags_class", "ArchiveFlags"),
        Some(FlagsClass::Old) => o.str("flags_class", "ArchiveFlagsOld"),
        None => o.null("flags_class"),
    }
    o.bool("block_alignment", b.block_alignment);
    o.bool("encrypted", b.encrypted);
    o.hex("hash", b.hash.as_ref().map(<[u8; 16]>::as_slice));
    o.opt_int("crc", b.crc);
    match &b.blocks_info {
        Some(r) => block_row(o.key("blocks_info"), r),
        None => o.null("blocks_info"),
    }
    match &b.blocks {
        Some(rows) => write_array(o.key("blocks"), rows, block_row),
        None => o.null("blocks"),
    }
    dir_rows(o.key("directory"), &b.directory);
    o.end();
}

fn web_l1(out: &mut String, w: &WebL1) {
    let mut o = Obj::new(out);
    o.str("kind", "web");
    o.name("signature", Some(w.signature.as_bytes()));
    o.str("packer", w.packer);
    dir_rows(o.key("directory"), &w.directory);
    o.end();
}

fn node_rows(out: &mut String, list: &NodeList) {
    use std::fmt::Write as _;
    fn opt<T: std::fmt::Display>(out: &mut String, v: Option<T>) {
        match v {
            Some(v) => {
                let _ = write!(out, "{v}");
            }
            None => out.push_str("null"),
        }
    }
    write_array(out, list.rows(), |out, r| {
        let _ = write!(out, "[{},", r.level);
        write_name(out, r.ty.as_bytes());
        out.push(',');
        write_name(out, r.name.as_bytes());
        let _ = write!(out, ",{},{},", r.byte_size, r.version);
        opt(out, r.type_flags);
        out.push(',');
        opt(out, r.variable_count);
        out.push(',');
        opt(out, r.index);
        out.push(',');
        opt(out, r.meta_flag);
        out.push(',');
        opt(out, r.ref_type_hash);
        out.push(']');
    });
}

fn type_record(out: &mut String, t: &TypeRecord) {
    use std::fmt::Write as _;
    let mut o = Obj::new(out);
    o.int("class_id", t.class_id);
    match t.is_stripped_type {
        Some(b) => o.bool("is_stripped_type", b),
        None => o.null("is_stripped_type"),
    }
    o.int("script_type_index", t.script_type_index);
    o.hex("script_id", t.script_id.as_ref().map(<[u8; 16]>::as_slice));
    o.hex(
        "old_type_hash",
        t.old_type_hash.as_ref().map(<[u8; 16]>::as_slice),
    );
    match &t.nodes {
        Some(list) => node_rows(o.key("nodes"), list),
        None => o.null("nodes"),
    }
    match &t.type_dependencies {
        Some(deps) => write_array(o.key("type_dependencies"), deps, |out, d| {
            let _ = write!(out, "{d}");
        }),
        None => o.null("type_dependencies"),
    }
    o.name("class_name", t.class_name.as_ref().map(RawStr::as_bytes));
    o.name("namespace", t.namespace.as_ref().map(RawStr::as_bytes));
    o.name("assembly", t.assembly.as_ref().map(RawStr::as_bytes));
    o.end();
}

fn serialized_l1(out: &mut String, s: &SerializedL1) {
    use std::fmt::Write as _;
    let mut o = Obj::new(out);
    o.str("kind", "serialized");
    {
        let h = &s.header;
        let mut m = Obj::new(o.key("header"));
        m.int("metadata_size", h.metadata_size);
        m.int("file_size", h.file_size);
        m.int("version", h.version);
        m.int("data_offset", h.data_offset);
        m.str("endian", if h.big_endian { ">" } else { "<" });
        m.hex("reserved", h.reserved.as_ref().map(<[u8; 3]>::as_slice));
        m.int("unknown", h.unknown);
        m.end();
    }
    o.name("unity_version", Some(s.unity_version.as_bytes()));
    match &s.version_packed {
        Some(p) => write_packed(o.key("version_packed"), p),
        None => o.null("version_packed"),
    }
    o.opt_int("target_platform", s.target_platform);
    o.bool("enable_type_tree", s.enable_type_tree);
    o.int("big_id_enabled", s.big_id_enabled);
    write_array(o.key("types"), &s.types, type_record);
    write_array(o.key("object_order"), &s.object_order, |out, p| {
        let _ = write!(out, "{p}");
    });
    write_array(o.key("script_types"), &s.script_types, |out, (f, id)| {
        let _ = write!(out, "[{f},{id}]");
    });
    write_array(o.key("externals"), &s.externals, |out, e| {
        out.push('[');
        match &e.temp_empty {
            Some(t) => write_name(out, t.as_bytes()),
            None => out.push_str("null"),
        }
        out.push(',');
        match &e.guid {
            Some(g) => write_hex_str(out, g),
            None => out.push_str("null"),
        }
        out.push(',');
        match e.kind {
            Some(k) => {
                let _ = write!(out, "{k}");
            }
            None => out.push_str("null"),
        }
        out.push(',');
        write_name(out, e.path.as_bytes());
        out.push(']');
    });
    match &s.ref_types {
        Some(t) => write_array(o.key("ref_types"), t, type_record),
        None => o.null("ref_types"),
    }
    o.name(
        "user_information",
        s.user_information.as_ref().map(RawStr::as_bytes),
    );
    write_array(o.key("container"), &s.container, |out, c| {
        out.push('[');
        match &c.path {
            Some(p) => write_name(out, p.as_bytes()),
            None => out.push_str("null"),
        }
        for v in [c.preload_index, c.preload_size, c.file_id, c.path_id] {
            match v {
                Some(v) => {
                    let _ = write!(out, ",{v}");
                }
                None => out.push_str(",null"),
            }
        }
        out.push(']');
    });
    o.end();
}

/// An `object` record's line; `file` is the owning SerializedFile's identity.
#[must_use]
pub fn object_line(file: &[RawStr], rec: &ObjectRecord) -> String {
    let mut out = String::new();
    let mut o = Obj::new(&mut out);
    o.str("rec", "object");
    write_identity(o.key("file"), file);
    o.int("path_id", rec.path_id);
    match &rec.l1 {
        Some(l1) => {
            let mut m = Obj::new(o.key("l1"));
            m.int("type_id", l1.type_id);
            m.int("class_id", l1.class_id);
            m.int("byte_start", l1.byte_start);
            m.int("byte_size", l1.byte_size);
            m.opt_int("is_destroyed", l1.is_destroyed);
            m.opt_int("is_stripped", l1.is_stripped);
            m.end();
        }
        None => o.null("l1"),
    }
    match &rec.l2 {
        Some(l2) => {
            let mut m = Obj::new(o.key("l2"));
            m.name("root_type", l2.root_type.as_ref().map(RawStr::as_bytes));
            match l2.source {
                Some(s) => m.str("source", s),
                None => m.null("source"),
            }
            match &l2.error {
                Some(e) => {
                    let mut x = Obj::new(m.key("error"));
                    x.str("kind", e.kind.as_str());
                    x.opt_int("bytes_read", e.bytes_read);
                    if let Some(n) = &e.note {
                        x.str("note", n);
                    }
                    x.end();
                }
                None => m.null("error"),
            }
            match &l2.value {
                Some(v) => m.raw("value", v),
                None => m.null("value"),
            }
            m.end();
        }
        None => o.null("l2"),
    }
    match &rec.l3 {
        Some(l3) => {
            let mut m = Obj::new(o.key("l3"));
            m.int("raw_size", l3.raw_size);
            m.hex("raw_sha256", Some(&l3.raw_sha256));
            match &l3.resave {
                Resave::Equal | Resave::Skipped | Resave::Error { .. } => {
                    m.str(
                        "resave",
                        match &l3.resave {
                            Resave::Equal => "equal",
                            Resave::Skipped => "skipped",
                            _ => "error",
                        },
                    );
                    m.null("resave_size");
                    m.null("resave_sha256");
                    m.null("first_diff");
                    if let Resave::Error { note: Some(n) } = &l3.resave {
                        m.str("note", n);
                    }
                }
                Resave::Differs {
                    size,
                    sha256,
                    first_diff,
                } => {
                    m.str("resave", "differs");
                    m.int("resave_size", size);
                    m.hex("resave_sha256", Some(sha256));
                    m.int("first_diff", first_diff);
                }
            }
            m.end();
        }
        None => o.null("l3"),
    }
    o.end();
    out
}
