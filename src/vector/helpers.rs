use crate::{QueryVector, SCALE};

#[inline]
pub fn quantize(value: f64) -> i16 {
    if value <= -1.0 {
        -SCALE
    } else if value <= 0.0 {
        0
    } else if value >= 1.0 {
        SCALE
    } else {
        (value * SCALE as f64).round() as i16
    }
}

#[inline(always)]
pub fn hash_bytes(value: &[u8]) -> u64 {
    let mut hash = 0x517cc1b727220a95u64;
    let mut i = 0;
    while i + 8 <= value.len() {
        let word = u64::from_le_bytes(value[i..i + 8].try_into().unwrap());
        hash = hash.rotate_left(5) ^ word;
        hash = hash.wrapping_mul(0x517cc1b727220a95);
        i += 8;
    }
    for &b in &value[i..] {
        hash = hash.rotate_left(5) ^ (b as u64);
        hash = hash.wrapping_mul(0x517cc1b727220a95);
    }
    hash
}

pub fn parse_mcc(mcc: &[u8]) -> i32 {
    if mcc.len() != 4 {
        return 0;
    }
    let a = mcc[0].wrapping_sub(b'0');
    let b = mcc[1].wrapping_sub(b'0');
    let c = mcc[2].wrapping_sub(b'0');
    let d = mcc[3].wrapping_sub(b'0');
    if a > 9 || b > 9 || c > 9 || d > 9 {
        return 0;
    }
    (a as i32) * 1000 + (b as i32) * 100 + (c as i32) * 10 + (d as i32)
}

pub fn mcc_risk(mcc: i32) -> f64 {
    match mcc {
        5411 => 0.15,
        5812 => 0.30,
        5912 => 0.20,
        5944 => 0.45,
        7801 => 0.80,
        7802 => 0.75,
        7995 => 0.85,
        4511 => 0.35,
        5311 => 0.25,
        5999 => 0.50,
        _ => 0.50,
    }
}

#[derive(Clone, Copy)]
pub struct ParsedDateTime {
    pub epoch_minute: i64,
    pub hour: i32,
    pub day_of_week: i32,
}

pub fn parse_datetime(iso: &[u8]) -> Result<ParsedDateTime, super::ParseError> {
    if iso.len() < 16 {
        return Err(super::ParseError::InvalidValue);
    }
    let y = parse4(iso, 0)?;
    let m = parse2(iso, 5)?;
    let d = parse2(iso, 8)?;
    let hh = parse2(iso, 11)?;
    let mm = parse2(iso, 14)?;

    let days = days_from_civil(y, m, d);
    let epoch_minute = days * 1_440 + (hh as i64) * 60 + (mm as i64);
    let day_of_week = ((days + 3) % 7) as i32;

    Ok(ParsedDateTime {
        epoch_minute,
        hour: hh,
        day_of_week,
    })
}

fn parse2(s: &[u8], offset: usize) -> Result<i32, super::ParseError> {
    if offset + 2 > s.len() {
        return Err(super::ParseError::InvalidValue);
    }
    let a = s[offset].wrapping_sub(b'0');
    let b = s[offset + 1].wrapping_sub(b'0');
    if a > 9 || b > 9 {
        return Err(super::ParseError::InvalidValue);
    }
    Ok((a as i32) * 10 + (b as i32))
}

fn parse4(s: &[u8], offset: usize) -> Result<i32, super::ParseError> {
    if offset + 4 > s.len() {
        return Err(super::ParseError::InvalidValue);
    }
    let a = s[offset].wrapping_sub(b'0');
    let b = s[offset + 1].wrapping_sub(b'0');
    let c = s[offset + 2].wrapping_sub(b'0');
    let d = s[offset + 3].wrapping_sub(b'0');
    if a > 9 || b > 9 || c > 9 || d > 9 {
        return Err(super::ParseError::InvalidValue);
    }
    Ok((a as i32) * 1000 + (b as i32) * 100 + (c as i32) * 10 + (d as i32))
}

fn days_from_civil(y: i32, m: i32, d: i32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u32;
    let shifted_month = m + if m > 2 { -3 } else { 9 };
    let doy = (153u32 * (shifted_month as u32) + 2) / 5 + (d as u32) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era as i64) * 146_097 + (doe as i64) - 719_468
}

pub fn finish_vector(
    out: &mut QueryVector,
    amount: f64,
    customer_avg_amount: f64,
    merchant_hash: u64,
    known_hashes: &[u64],
) {
    out[2] = if customer_avg_amount > 0.0 {
        quantize((amount / customer_avg_amount) / 10.0)
    } else {
        SCALE
    };

    let mut known = false;
    for &h in known_hashes {
        if h == merchant_hash {
            known = true;
            break;
        }
    }
    out[11] = if known { 0 } else { SCALE };
}

pub fn fast_parse_f64(s: &[u8]) -> Option<f64> {
    if s.is_empty() {
        return None;
    }
    let mut i = 0;
    let mut neg = false;
    if s[0] == b'-' {
        neg = true;
        i += 1;
    } else if s[0] == b'+' {
        i += 1;
    }

    let mut val = 0u64;
    let mut decimal_point = None;
    let mut digits = 0;

    while i < s.len() {
        let b = s[i];
        if b == b'.' {
            if decimal_point.is_some() {
                return None;
            }
            decimal_point = Some(digits);
        } else if b >= b'0' && b <= b'9' {
            if digits >= 18 {
                return None;
            }
            val = val * 10 + (b - b'0') as u64;
            digits += 1;
        } else {
            return None;
        }
        i += 1;
    }

    if digits == 0 {
        return None;
    }

    let scale = match decimal_point {
        Some(pos) => digits - pos,
        None => 0,
    };

    let factor = match scale {
        0 => 1.0,
        1 => 0.1,
        2 => 0.01,
        3 => 0.001,
        4 => 0.0001,
        5 => 0.00001,
        6 => 0.000001,
        7 => 0.0000001,
        8 => 0.00000001,
        9 => 0.000000001,
        10 => 0.0000000001,
        11 => 0.00000000001,
        12 => 0.000000000001,
        13 => 0.0000000000001,
        14 => 0.00000000000001,
        15 => 0.000000000000001,
        16 => 0.0000000000000001,
        17 => 0.00000000000000001,
        _ => return None,
    };

    let mut result = val as f64 * factor;
    if neg {
        result = -result;
    }
    Some(result)
}

pub fn read_double_at(json: &[u8], start: usize) -> Result<f64, super::ParseError> {
    let s = &json[start..];
    let mut end = 0usize;
    let mut seen_dot = false;
    let mut seen_digit = false;
    for &b in s {
        match b {
            b'0'..=b'9' => {
                seen_digit = true;
                end += 1;
            }
            b'-' | b'+' if end == 0 => end += 1,
            b'.' if !seen_dot => {
                seen_dot = true;
                end += 1;
            }
            b'e' | b'E' if seen_digit => {
                end += 1;
                if s.get(end).copied() == Some(b'-') || s.get(end).copied() == Some(b'+') {
                    end += 1;
                }
                while s.get(end).map_or(false, |&b| b.is_ascii_digit()) {
                    end += 1;
                }
                break;
            }
            _ => break,
        }
    }
    if end == 0 {
        return Err(super::ParseError::InvalidValue);
    }
    let token = &s[..end];
    if let Some(val) = fast_parse_f64(token) {
        Ok(val)
    } else {
        let text = std::str::from_utf8(token).map_err(|_| super::ParseError::InvalidValue)?;
        text.parse::<f64>()
            .map_err(|_| super::ParseError::InvalidValue)
    }
}

pub fn read_int_at(json: &[u8], start: usize) -> Result<i32, super::ParseError> {
    let s = &json[start..];
    let mut end = 0usize;
    if s.first().copied() == Some(b'-') {
        end += 1;
    }
    while s.get(end).map_or(false, |&b| b.is_ascii_digit()) {
        end += 1;
    }
    if end == 0 || (end == 1 && s[0] == b'-') {
        return Err(super::ParseError::InvalidValue);
    }
    let token = &s[..end];
    let mut val = 0i32;
    let mut neg = false;
    let mut i = 0;
    if token[0] == b'-' {
        neg = true;
        i += 1;
    }
    for &b in &token[i..] {
        val = val.wrapping_mul(10).wrapping_add((b - b'0') as i32);
    }
    if neg {
        val = -val;
    }
    Ok(val)
}

pub fn read_bool_at(json: &[u8], start: usize) -> Result<bool, super::ParseError> {
    let s = &json[start..];
    if s.starts_with(b"true") {
        Ok(true)
    } else if s.starts_with(b"false") {
        Ok(false)
    } else {
        Err(super::ParseError::InvalidValue)
    }
}

pub fn read_string_at<'a>(json: &'a [u8], start: usize) -> Result<&'a [u8], super::ParseError> {
    if start >= json.len() || json[start] != b'"' {
        return Err(super::ParseError::InvalidValue);
    }
    let content_start = start + 1;
    let mut escaped = false;
    for i in content_start..json.len() {
        let b = json[i];
        if escaped {
            escaped = false;
            continue;
        }
        if b == b'\\' {
            escaped = true;
            continue;
        }
        if b == b'"' {
            return Ok(&json[content_start..i]);
        }
    }
    Err(super::ParseError::InvalidValue)
}

pub fn is_json_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\r' | b'\t')
}
