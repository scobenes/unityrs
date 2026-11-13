//! At most sixteen heads and a fixed heap. Every consumed file is closed
//! before unlink; input and output catalogs overlap only for one generation.
use super::{
    Control, FAN_IN, Order, Point, catalog, compare,
    format::{Reader, Writer},
};
use crate::{
    phase::Error,
    spool::workspace::{Sealed, Workspace},
};
use std::{cmp::Ordering, sync::Arc};
use unityrs_core::limits::Meter;

struct Input<'a> {
    reader: Reader<'a>,
    file: Sealed,
}
struct Heap {
    entries: [usize; FAN_IN],
    len: usize,
}
impl Heap {
    fn new() -> Self {
        Self {
            entries: [0; FAN_IN],
            len: 0,
        }
    }
    fn entry(&self, i: usize) -> Result<usize, Error> {
        self.entries.get(i).copied().ok_or_else(Error::internal)
    }
    fn push(
        &mut self,
        value: usize,
        inputs: &[Option<Input<'_>>; FAN_IN],
        order: &dyn Order,
    ) -> Result<(), Error> {
        *self.entries.get_mut(self.len).ok_or_else(Error::internal)? = value;
        let mut child = self.len;
        self.len += 1;
        while child > 0 {
            let parent = (child - 1) / 2;
            if !less(self.entry(child)?, self.entry(parent)?, inputs, order)? {
                break;
            }
            self.entries.swap(child, parent);
            child = parent;
        }
        Ok(())
    }
    fn pop(
        &mut self,
        inputs: &[Option<Input<'_>>; FAN_IN],
        order: &dyn Order,
    ) -> Result<Option<usize>, Error> {
        if self.len == 0 {
            return Ok(None);
        }
        let first = self.entry(0)?;
        self.len -= 1;
        *self.entries.get_mut(0).ok_or_else(Error::internal)? = self.entry(self.len)?;
        let mut parent = 0;
        loop {
            let left = parent * 2 + 1;
            if left >= self.len {
                break;
            }
            let right = left + 1;
            let child = if right < self.len
                && less(self.entry(right)?, self.entry(left)?, inputs, order)?
            {
                right
            } else {
                left
            };
            if !less(self.entry(child)?, self.entry(parent)?, inputs, order)? {
                break;
            }
            self.entries.swap(parent, child);
            parent = child;
        }
        Ok(Some(first))
    }
}
fn input<'a, 'b>(
    inputs: &'a [Option<Input<'b>>; FAN_IN],
    index: usize,
) -> Result<&'a Input<'b>, Error> {
    inputs
        .get(index)
        .and_then(Option::as_ref)
        .ok_or_else(Error::internal)
}
fn less(
    a: usize,
    b: usize,
    inputs: &[Option<Input<'_>>; FAN_IN],
    order: &dyn Order,
) -> Result<bool, Error> {
    let compared = compare(
        order,
        input(inputs, a)?.reader.head(),
        input(inputs, b)?.reader.head(),
    );
    Ok(compared.then_with(|| a.cmp(&b)) == Ordering::Less)
}
fn group(
    files: &mut [Option<Sealed>; FAN_IN],
    order: &dyn Order,
    workspace: &mut Workspace,
    meter: &Arc<Meter>,
    control: &Control<'_>,
) -> Result<Sealed, Error> {
    let mut inputs: [Option<Input<'_>>; FAN_IN] = std::array::from_fn(|_| None);
    let mut heap = Heap::new();
    for (i, file) in files.iter_mut().enumerate() {
        let Some(file) = file.take() else {
            break;
        };
        let mut reader = Reader::open(workspace, &file, order, meter, control.stopped)?;
        control.at(Point::BeforeRead)?;
        if reader.next(control.stopped)?.is_none() {
            return Err(Error::internal());
        }
        *inputs.get_mut(i).ok_or_else(Error::internal)? = Some(Input { reader, file });
        heap.push(i, &inputs, order)?;
    }
    let mut output = Writer::new(workspace, order.tag(), meter, control)?;
    while let Some(index) = heap.pop(&inputs, order)? {
        output.frame(input(&inputs, index)?.reader.head(), control)?;
        let slot = inputs.get_mut(index).ok_or_else(Error::internal)?;
        control.at(Point::BeforeRead)?;
        if slot
            .as_mut()
            .ok_or_else(Error::internal)?
            .reader
            .next(control.stopped)?
            .is_some()
        {
            heap.push(index, &inputs, order)?;
        } else {
            let Input { reader, file } = slot.take().ok_or_else(Error::internal)?;
            drop(reader);
            catalog::retire(workspace, file, control)?;
        }
    }
    output.finish(control)
}

pub(crate) fn generations(
    mut current: catalog::Catalog,
    order: &dyn Order,
    workspace: &mut Workspace,
    meter: &Arc<Meter>,
    fan_in: usize,
    control: &Control<'_>,
) -> Result<(Option<Sealed>, u32), Error> {
    let mut generations: u32 = 0;
    while current.count > 1 {
        control.at(Point::Pass)?;
        let mut read = catalog::Reader::open(workspace, &current, control)?;
        let mut write = catalog::Writer::new(workspace, control)?;
        loop {
            control.at(Point::MergeGroup)?;
            let mut files: [Option<Sealed>; FAN_IN] = std::array::from_fn(|_| None);
            let mut count = 0;
            for slot in files.iter_mut().take(fan_in) {
                *slot = read.next(control)?;
                if slot.is_none() {
                    break;
                }
                count += 1;
            }
            if count == 0 {
                break;
            }
            let output = if count == 1 {
                files
                    .get_mut(0)
                    .and_then(Option::take)
                    .ok_or_else(Error::internal)?
            } else {
                group(&mut files, order, workspace, meter, control)?
            };
            write.push(output, control)?;
        }
        drop(read);
        catalog::retire(workspace, current.file, control)?;
        current = write.finish(control)?;
        generations = generations.checked_add(1).ok_or_else(Error::internal)?;
    }
    control.at(Point::Pass)?;
    let mut read = catalog::Reader::open(workspace, &current, control)?;
    let file = read.next(control)?;
    if read.next(control)?.is_some() {
        return Err(Error::internal());
    }
    drop(read);
    // Even a single initial run passes through its disk descriptor. Validate
    // the capability and complete stream before returning a ready handoff.
    if let Some(file) = &file {
        let mut reader = Reader::open(workspace, file, order, meter, control.stopped)?;
        loop {
            control.at(Point::BeforeRead)?;
            if reader.next(control.stopped)?.is_none() {
                break;
            }
        }
    }
    catalog::retire(workspace, current.file, control)?;
    Ok((file, generations))
}
