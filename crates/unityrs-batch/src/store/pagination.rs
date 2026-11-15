//! Fixed-storage LIST pagination fields, before the dependency drops `IsTruncated`.
use super::scan::Breach;

#[derive(Clone, Debug, Default)]
pub(super) struct Pagination {
    value: [u8; 5],
    len: usize,
    trailing_space: bool,
    entity: [u8; 12],
    entity_len: Option<usize>,
    pub(super) capturing: bool,
    truncated: Option<bool>,
    pub(super) token: bool,
}
impl Pagination {
    pub(super) const fn new() -> Self {
        Self {
            value: [0; 5],
            len: 0,
            trailing_space: false,
            entity: [0; 12],
            entity_len: None,
            capturing: false,
            truncated: None,
            token: false,
        }
    }
    pub(super) fn start(&mut self) -> Result<(), Breach> {
        if self.truncated.is_some() || self.capturing {
            return Err(Breach);
        }
        self.capturing = true;
        Ok(())
    }
    pub(super) fn text(&mut self, b: u8, cdata: bool) -> Result<(), Breach> {
        if !self.capturing {
            return Ok(());
        }
        if !cdata {
            if let Some(len) = self.entity_len {
                if b == b';' {
                    let raw = std::str::from_utf8(self.entity.get(..len).ok_or(Breach)?)
                        .map_err(|_| Breach)?;
                    let n = if let Some(hex) = raw.strip_prefix("#x") {
                        u8::from_str_radix(hex, 16).map_err(|_| Breach)?
                    } else {
                        raw.strip_prefix('#')
                            .ok_or(Breach)?
                            .parse::<u8>()
                            .map_err(|_| Breach)?
                    };
                    self.entity_len = None;
                    return self.character(n);
                }
                *self.entity.get_mut(len).ok_or(Breach)? = b;
                self.entity_len = Some(len + 1);
                return Ok(());
            }
            if b == b'&' {
                self.entity_len = Some(0);
                return Ok(());
            }
        }
        self.character(b)
    }
    fn character(&mut self, b: u8) -> Result<(), Breach> {
        if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
            self.trailing_space |= self.len != 0;
        } else {
            if self.trailing_space {
                return Err(Breach);
            }
            *self.value.get_mut(self.len).ok_or(Breach)? = b;
            self.len += 1;
        }
        Ok(())
    }
    pub(super) fn end(&mut self) -> Result<(), Breach> {
        if self.entity_len.is_some() {
            return Err(Breach);
        }
        self.truncated = Some(match self.value.get(..self.len).ok_or(Breach)? {
            b"true" | b"1" => true,
            b"false" | b"0" => false,
            _ => return Err(Breach),
        });
        self.capturing = false;
        Ok(())
    }
    pub(super) fn valid(&self) -> bool {
        !self.capturing && self.truncated.unwrap_or(false) == self.token
    }
    pub(super) const fn truncated(&self) -> Option<bool> {
        self.truncated
    }
}
