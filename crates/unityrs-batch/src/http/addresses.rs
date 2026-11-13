//! First-sixteen resolver results in fixed storage and original order.
use std::net::SocketAddr;
pub const MAX_ADDRESSES: usize = 16;
/// Fixed storage preserves resolver order without allocating another list.
#[derive(Clone, Debug, Default)]
pub struct Addresses {
    entries: [Option<SocketAddr>; MAX_ADDRESSES],
    len: usize,
}
impl Addresses {
    #[must_use]
    pub fn first(addresses: impl IntoIterator<Item = SocketAddr>) -> Self {
        let mut result = Self::default();
        for (slot, address) in result.entries.iter_mut().zip(addresses) {
            *slot = Some(address);
            result.len += 1;
        }
        result
    }
    pub fn iter(&self) -> impl Iterator<Item = SocketAddr> + '_ {
        self.entries.iter().flatten().copied()
    }
    pub(crate) fn get(&self, index: usize) -> Option<SocketAddr> {
        self.entries.get(index).copied().flatten()
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}
