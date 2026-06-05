use crate::QueryVector;

mod compact;
mod helpers;
mod serde_fallback;
mod single_pass;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    MissingField,
    InvalidValue,
    InvalidFormat,
}

use compact::try_parse_compact_ordered;
use serde_fallback::try_parse_serde;
use single_pass::try_parse_single_pass;

pub fn parse_query(payload: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
    out.fill(0);

    if try_parse_compact_ordered(payload, out).is_ok() {
        return Ok(());
    }

    out.fill(0);
    if try_parse_single_pass(payload, out).is_ok() {
        return Ok(());
    }

    out.fill(0);
    try_parse_serde(payload, out)
}
