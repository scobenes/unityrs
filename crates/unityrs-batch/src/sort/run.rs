//! Packed arena and fixed-width index. Exact allocations avoid growth peaks;
//! unstable in-place sorting allocates no scratch or per-comparison keys.
use super::{
    Control, Options, Order, Point, Source, catalog, compare,
    format::{Frame, Writer},
};
use crate::{
    phase::{Buffer, Error},
    spool::workspace::Workspace,
};
use std::sync::Arc;
use unityrs_core::limits::Meter;

#[derive(Clone, Copy)]
struct Index {
    start: u32,
    length: u32,
}
fn payload(arena: &[u8], index: Index) -> &[u8] {
    let start = usize::try_from(index.start).unwrap_or(usize::MAX);
    let length = usize::try_from(index.length).unwrap_or(usize::MAX);
    // Indices are constructed only from checked arena offsets, never disk.
    arena
        .get(start..start.saturating_add(length))
        .unwrap_or_default()
}
pub(crate) fn generate(
    source: &mut dyn Source,
    order: &dyn Order,
    workspace: &mut Workspace,
    meter: &Arc<Meter>,
    options: Options,
    control: &Control<'_>,
) -> Result<catalog::Catalog, Error> {
    control.at(Point::Pass)?;
    // Divide the ceiling into two exact allocations. Large allocations can
    // each round up one page in heap_cost; leave both pages inside the cap.
    let padding = if options.run_bytes >= 16 * 1024 {
        8192
    } else {
        64
    };
    let room = options
        .run_bytes
        .checked_sub(padding)
        .ok_or_else(Error::internal)?;
    let arena_bytes = room * 3 / 4;
    let mut arena = Buffer::new(meter, arena_bytes)?;
    let mut index = Buffer::<Index>::new(meter, (room - arena_bytes) / size_of::<Index>())?;
    let charged = arena
        .charged()
        .checked_add(index.charged())
        .ok_or_else(Error::internal)?;
    if charged > u64::try_from(options.run_bytes).map_err(|_| Error::internal())? {
        return Err(Error::internal());
    }
    control.hooks.run_storage(charged);
    let mut frame = Frame::new(meter)?;
    let mut inventory = catalog::Writer::new(workspace, control)?;
    let mut pending = false;
    let mut eof = false;
    loop {
        while !eof
            && u64::try_from(index.len()).map_err(|_| Error::internal())? < options.run_records
        {
            if !pending {
                control.at(Point::BeforeRead)?;
                if source.next(&mut frame, control.stopped)? {
                    order.validate(frame.bytes())?;
                } else {
                    eof = true;
                    break;
                }
            }
            pending = true;
            if frame.bytes().len() > arena_bytes - arena.len()
                || index.len() == (room - arena_bytes) / size_of::<Index>()
            {
                break;
            }
            index.push(Index {
                start: u32::try_from(arena.len()).map_err(|_| Error::internal())?,
                length: u32::try_from(frame.bytes().len()).map_err(|_| Error::internal())?,
            })?;
            arena.extend(frame.bytes())?;
            pending = false;
        }
        if index.is_empty() {
            if pending {
                return Err(Error::internal());
            }
            break;
        }
        control.at(Point::BeforeSort)?;
        index.sort_unstable_by(|a, b| compare(order, payload(&arena, *a), payload(&arena, *b)));
        control.at(Point::AfterSort)?;
        let mut writer = Writer::new(workspace, order.tag(), meter, control)?;
        for entry in index.iter() {
            writer.frame(payload(&arena, *entry), control)?;
        }
        let sealed = writer.finish(control)?;
        control.at(Point::RunSealed)?;
        inventory.push(sealed, control)?;
        index.clear();
        arena.clear();
        if eof {
            break;
        }
    }
    inventory.finish(control)
}
