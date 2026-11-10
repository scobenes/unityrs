//! A stored typetree (`unityrs_testkit::fuzz::blob_case`): the blob format
//! (`parse_blob`, format 10 and from 12) or the old recursive one
//! (`parse_old`), as the format says, in either byte order, then the rows
//! as a node list and the list compiled, as a file's type is.
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use unityrs_core::Bytes;
use unityrs_core::stream::{Endian, Reader};
use unityrs_core::typetree::parse::{is_blob_format, parse_blob, parse_old};
use unityrs_core::typetree::{NodeList, Tree};

fuzz_target!(|data: &[u8]| {
    let [format, flags, tree @ ..] = data else {
        return;
    };
    let format = u32::from(*format);
    let endian = if flags & 1 == 1 {
        Endian::Big
    } else {
        Endian::Little
    };
    let ctx = unityrs_fuzz::context();
    let meter = unityrs_fuzz::meter(&ctx);
    let data = Bytes::copy_from_slice(tree);
    let mut r = Reader::new(&data, endian);
    let rows = if is_blob_format(format) {
        parse_blob(&mut r, &data, format, &meter, &ctx.limits, None)
    } else {
        parse_old(&mut r, &data, format, &meter, &ctx.limits)
    };
    if let Ok(Ok(list)) = rows.map(NodeList::from_rows) {
        let _ = Tree::compile(Arc::new(list), &ctx.limits);
    }
});
