use crate::QueryVector;

use super::ParseError;
use super::helpers::{finish_vector, hash_bytes, mcc_risk, parse_datetime, parse_mcc, quantize};

const INV_10_000: f64 = 1.0 / 10_000.0;
const INV_12: f64 = 1.0 / 12.0;
const INV_23: f64 = 1.0 / 23.0;
const INV_6: f64 = 1.0 / 6.0;
const INV_1_440: f64 = 1.0 / 1_440.0;
const INV_1_000: f64 = 1.0 / 1_000.0;
const INV_20: f64 = 1.0 / 20.0;

#[inline(always)]
fn consume(json: &[u8], i: &mut usize, expected: &[u8]) -> Result<(), ParseError> {
    if json.get(*i..*i + expected.len()) == Some(expected) {
        *i += expected.len();
        Ok(())
    } else {
        Err(ParseError::InvalidFormat)
    }
}

fn read_compact_string<'a>(json: &'a [u8], i: &mut usize) -> Result<&'a [u8], ParseError> {
    let start = *i;
    let mut j = start;
    while j < json.len() {
        match json[j] {
            b'"' => {
                *i = j + 1;
                return Ok(&json[start..j]);
            }
            b'\\' => return Err(ParseError::InvalidFormat),
            _ => j += 1,
        }
    }
    Err(ParseError::InvalidFormat)
}

fn read_compact_f64(json: &[u8], i: &mut usize) -> Result<f64, ParseError> {
    let start = *i;
    if matches!(json.get(*i).copied(), Some(b'-' | b'+')) {
        *i += 1;
    }
    let mut seen_dot = false;
    let mut seen_digit = false;
    while let Some(b) = json.get(*i).copied() {
        match b {
            b'0'..=b'9' => {
                seen_digit = true;
                *i += 1;
            }
            b'.' if !seen_dot => {
                seen_dot = true;
                *i += 1;
            }
            _ => break,
        }
    }
    if !seen_digit {
        return Err(ParseError::InvalidValue);
    }
    super::helpers::fast_parse_f64(&json[start..*i]).ok_or(ParseError::InvalidValue)
}

fn read_compact_i32(json: &[u8], i: &mut usize) -> Result<i32, ParseError> {
    let start = *i;
    if json.get(*i).copied() == Some(b'-') {
        *i += 1;
    }
    let mut value = 0i32;
    let mut seen_digit = false;
    while let Some(b) = json.get(*i).copied() {
        if !b.is_ascii_digit() {
            break;
        }
        seen_digit = true;
        value = value.wrapping_mul(10).wrapping_add((b - b'0') as i32);
        *i += 1;
    }
    if !seen_digit {
        return Err(ParseError::InvalidValue);
    }
    if json[start] == b'-' {
        value = -value;
    }
    Ok(value)
}

fn read_compact_bool(json: &[u8], i: &mut usize) -> Result<bool, ParseError> {
    if json.get(*i..*i + 4) == Some(b"true") {
        *i += 4;
        Ok(true)
    } else if json.get(*i..*i + 5) == Some(b"false") {
        *i += 5;
        Ok(false)
    } else {
        Err(ParseError::InvalidValue)
    }
}

fn read_compact_known_merchants(
    json: &[u8],
    i: &mut usize,
    hashes: &mut [u64; 64],
) -> Result<usize, ParseError> {
    consume(json, i, b"[")?;
    let mut count = 0usize;
    if json.get(*i).copied() == Some(b']') {
        *i += 1;
        return Ok(0);
    }
    loop {
        consume(json, i, b"\"")?;
        let value = read_compact_string(json, i)?;
        if count < hashes.len() {
            hashes[count] = hash_bytes(value);
            count += 1;
        }
        match json.get(*i).copied() {
            Some(b',') => *i += 1,
            Some(b']') => {
                *i += 1;
                return Ok(count);
            }
            _ => return Err(ParseError::InvalidFormat),
        }
    }
}

pub fn try_parse_compact_ordered(json: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
    out.fill(0);

    let mut known_hashes = [0u64; 64];
    let mut i = 0usize;

    consume(json, &mut i, b"{\"id\":\"")?;
    let _ = read_compact_string(json, &mut i)?;

    consume(json, &mut i, b",\"transaction\":{\"amount\":")?;
    let amount = read_compact_f64(json, &mut i)?;
    out[0] = quantize(amount * INV_10_000);

    consume(json, &mut i, b",\"installments\":")?;
    let installments = read_compact_i32(json, &mut i)?;
    out[1] = quantize(installments as f64 * INV_12);

    consume(json, &mut i, b",\"requested_at\":\"")?;
    let requested_at = read_compact_string(json, &mut i)?;
    let requested = parse_datetime(requested_at)?;
    let requested_minute = requested.epoch_minute;
    out[3] = quantize(requested.hour as f64 * INV_23);
    out[4] = quantize(requested.day_of_week as f64 * INV_6);

    consume(json, &mut i, b"},\"customer\":{\"avg_amount\":")?;
    let customer_avg_amount = read_compact_f64(json, &mut i)?;

    consume(json, &mut i, b",\"tx_count_24h\":")?;
    let tx_count_24h = read_compact_i32(json, &mut i)?;
    out[8] = quantize(tx_count_24h as f64 * INV_20);

    consume(json, &mut i, b",\"known_merchants\":")?;
    let known_count = read_compact_known_merchants(json, &mut i, &mut known_hashes)?;

    consume(json, &mut i, b"},\"merchant\":{\"id\":\"")?;
    let merchant_id = read_compact_string(json, &mut i)?;
    let merchant_hash = hash_bytes(merchant_id);

    consume(json, &mut i, b",\"mcc\":\"")?;
    let mcc = read_compact_string(json, &mut i)?;
    out[12] = quantize(mcc_risk(parse_mcc(mcc)));

    consume(json, &mut i, b",\"avg_amount\":")?;
    let merchant_avg_amount = read_compact_f64(json, &mut i)?;
    out[13] = quantize(merchant_avg_amount * INV_10_000);

    consume(json, &mut i, b"},\"terminal\":{\"is_online\":")?;
    out[9] = if read_compact_bool(json, &mut i)? {
        crate::SCALE
    } else {
        0
    };

    consume(json, &mut i, b",\"card_present\":")?;
    out[10] = if read_compact_bool(json, &mut i)? {
        crate::SCALE
    } else {
        0
    };

    consume(json, &mut i, b",\"km_from_home\":")?;
    let km_from_home = read_compact_f64(json, &mut i)?;
    out[7] = quantize(km_from_home * INV_1_000);

    consume(json, &mut i, b"},\"last_transaction\":")?;
    if json.get(i..i + 5) == Some(b"null}") {
        i += 5;
        out[5] = -crate::SCALE;
        out[6] = -crate::SCALE;
    } else {
        consume(json, &mut i, b"{\"timestamp\":\"")?;
        let timestamp = read_compact_string(json, &mut i)?;
        let last_minute = parse_datetime(timestamp)?.epoch_minute;
        let minutes_diff = requested_minute.saturating_sub(last_minute);
        out[5] = quantize(minutes_diff as f64 * INV_1_440);

        consume(json, &mut i, b",\"km_from_current\":")?;
        let km_from_current = read_compact_f64(json, &mut i)?;
        out[6] = quantize(km_from_current * INV_1_000);
        consume(json, &mut i, b"}}")?;
    }

    if i != json.len() {
        return Err(ParseError::InvalidFormat);
    }

    finish_vector(
        out,
        amount,
        customer_avg_amount,
        merchant_hash,
        &known_hashes[..known_count],
    );
    Ok(())
}
