use crate::{QueryVector, SCALE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    MissingField,
    InvalidValue,
    InvalidFormat,
}

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, Visitor};
use std::borrow::Cow;
use std::fmt;

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
    fast_parse_f64(&json[start..*i]).ok_or(ParseError::InvalidValue)
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

fn try_parse_compact_ordered(json: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
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
        SCALE
    } else {
        0
    };

    consume(json, &mut i, b",\"card_present\":")?;
    out[10] = if read_compact_bool(json, &mut i)? {
        SCALE
    } else {
        0
    };

    consume(json, &mut i, b",\"km_from_home\":")?;
    let km_from_home = read_compact_f64(json, &mut i)?;
    out[7] = quantize(km_from_home * INV_1_000);

    consume(json, &mut i, b"},\"last_transaction\":")?;
    if json.get(i..i + 5) == Some(b"null}") {
        i += 5;
        out[5] = -SCALE;
        out[6] = -SCALE;
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

#[inline]
fn quantize(value: f64) -> i16 {
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
fn skip_ws(json: &[u8], mut i: usize) -> usize {
    while i < json.len() && is_json_whitespace(json[i]) {
        i += 1;
    }
    i
}

fn skip_string(json: &[u8], i: usize) -> Result<usize, ParseError> {
    if json.get(i).copied() != Some(b'"') {
        return Err(ParseError::InvalidFormat);
    }
    let mut escaped = false;
    let mut j = i + 1;
    while j < json.len() {
        let b = json[j];
        if escaped {
            escaped = false;
            j += 1;
            continue;
        }
        if b == b'\\' {
            escaped = true;
            j += 1;
            continue;
        }
        if b == b'"' {
            return Ok(j + 1);
        }
        j += 1;
    }
    Err(ParseError::InvalidFormat)
}

fn skip_number(json: &[u8], i: usize) -> Result<usize, ParseError> {
    let mut j = i;
    // sign
    if matches!(json.get(j).copied(), Some(b'-' | b'+')) {
        j += 1;
    }
    let mut seen_dot = false;
    let mut seen_digit = false;
    while j < json.len() {
        let b = json[j];
        match b {
            b'0'..=b'9' => {
                seen_digit = true;
                j += 1;
            }
            b'.' if !seen_dot => {
                seen_dot = true;
                j += 1;
            }
            b'e' | b'E' if seen_digit => {
                j += 1;
                if matches!(json.get(j).copied(), Some(b'-' | b'+')) {
                    j += 1;
                }
                while j < json.len() {
                    let d = json[j];
                    if matches!(d, b'0'..=b'9') {
                        j += 1;
                    } else {
                        break;
                    }
                }
                break;
            }
            _ => break,
        }
    }
    if !seen_digit {
        Err(ParseError::InvalidValue)
    } else {
        Ok(j)
    }
}

fn skip_literal(json: &[u8], i: usize, lit: &[u8]) -> Result<usize, ParseError> {
    if json
        .get(i..i + lit.len())
        .ok_or(ParseError::InvalidFormat)?
        == lit
    {
        Ok(i + lit.len())
    } else {
        Err(ParseError::InvalidFormat)
    }
}

fn skip_value(json: &[u8], i: usize) -> Result<usize, ParseError> {
    let i = skip_ws(json, i);
    match json.get(i).copied() {
        Some(b'"') => skip_string(json, i),
        Some(b'-' | b'+' | b'0'..=b'9') => skip_number(json, i),
        Some(b't') => skip_literal(json, i, b"true"),
        Some(b'f') => skip_literal(json, i, b"false"),
        Some(b'n') => skip_literal(json, i, b"null"),
        Some(b'{') => {
            let mut depth = 1usize;
            let mut j = i + 1;
            while j < json.len() && depth > 0 {
                match json[j] {
                    b'"' => {
                        j = skip_string(json, j)?;
                    }
                    b'{' => {
                        depth += 1;
                        j += 1;
                    }
                    b'}' => {
                        depth -= 1;
                        j += 1;
                    }
                    _ => j += 1,
                }
            }
            if depth == 0 {
                Ok(j)
            } else {
                Err(ParseError::InvalidFormat)
            }
        }
        Some(b'[') => {
            let mut depth = 1usize;
            let mut j = i + 1;
            while j < json.len() && depth > 0 {
                match json[j] {
                    b'"' => {
                        j = skip_string(json, j)?;
                    }
                    b'[' => {
                        depth += 1;
                        j += 1;
                    }
                    b']' => {
                        depth -= 1;
                        j += 1;
                    }
                    _ => j += 1,
                }
            }
            if depth == 0 {
                Ok(j)
            } else {
                Err(ParseError::InvalidFormat)
            }
        }
        _ => Err(ParseError::InvalidFormat),
    }
}

fn try_parse_single_pass(json: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
    out.fill(0);

    let mut amount = 0.0f64;
    let mut customer_avg_amount = 0.0f64;
    let mut merchant_hash = 0u64;
    let mut known_hashes = [0u64; 64];
    let mut known_count = 0usize;
    let mut requested_minute = 0i64;

    let mut has_amount = false;
    let mut has_installments = false;
    let mut has_requested_at = false;
    let mut has_customer_avg_amount = false;
    let mut has_tx_count_24h = false;
    let mut has_known_merchants = false;
    let mut has_merchant_id = false;
    let mut has_mcc = false;
    let mut has_merchant_avg_amount = false;
    let mut has_is_online = false;
    let mut has_card_present = false;
    let mut has_km_from_home = false;
    let mut has_last_transaction = false;
    let mut last_timestamp = None::<i64>;
    let mut last_km_from_current = None::<f64>;

    let mut i = skip_ws(json, 0);
    if json.get(i).copied() != Some(b'{') {
        return Err(ParseError::InvalidFormat);
    }
    i += 1;

    loop {
        i = skip_ws(json, i);
        if i >= json.len() {
            return Err(ParseError::InvalidFormat);
        }
        if json.get(i).copied() == Some(b'}') {
            break;
        }

        if json.get(i).copied() != Some(b'"') {
            return Err(ParseError::InvalidFormat);
        }
        let key_start = i + 1;
        let end = skip_string(json, i)?;
        let key_end = end - 1;
        let key = &json[key_start..key_end];

        i = skip_ws(json, end);
        if json.get(i).copied() != Some(b':') {
            return Err(ParseError::InvalidFormat);
        }
        i += 1;

        let value_start = skip_ws(json, i);

        if key == b"transaction" {
            i = skip_ws(json, value_start);
            if json.get(i).copied() != Some(b'{') {
                return Err(ParseError::InvalidFormat);
            }
            i += 1;
            loop {
                i = skip_ws(json, i);
                if json.get(i).copied() == Some(b'}') {
                    i += 1;
                    break;
                }
                if json.get(i).copied() != Some(b'"') {
                    return Err(ParseError::InvalidFormat);
                }
                let k_start = i + 1;
                let k_end = skip_string(json, i)?;
                let k = &json[k_start..(k_end - 1)];

                i = skip_ws(json, k_end);
                if json.get(i).copied() != Some(b':') {
                    return Err(ParseError::InvalidFormat);
                }
                i += 1;
                let v_start = skip_ws(json, i);

                if k == b"amount" {
                    amount = read_double_at(json, v_start)?;
                    out[0] = quantize(amount * INV_10_000);
                    has_amount = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"installments" {
                    let v = read_int_at(json, v_start)?;
                    out[1] = quantize(v as f64 * INV_12);
                    has_installments = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"requested_at" {
                    let s = read_string_at(json, v_start)?;
                    let parsed = parse_datetime(s)?;
                    requested_minute = parsed.epoch_minute;
                    out[3] = quantize(parsed.hour as f64 * INV_23);
                    out[4] = quantize(parsed.day_of_week as f64 * INV_6);
                    has_requested_at = true;
                    i = skip_value(json, v_start)?;
                } else {
                    i = skip_value(json, v_start)?;
                }

                i = skip_ws(json, i);
                if json.get(i).copied() == Some(b',') {
                    i += 1;
                }
            }
        } else if key == b"customer" {
            i = skip_ws(json, value_start);
            if json.get(i).copied() != Some(b'{') {
                return Err(ParseError::InvalidFormat);
            }
            i += 1;
            loop {
                i = skip_ws(json, i);
                if json.get(i).copied() == Some(b'}') {
                    i += 1;
                    break;
                }
                if json.get(i).copied() != Some(b'"') {
                    return Err(ParseError::InvalidFormat);
                }
                let k_start = i + 1;
                let k_end = skip_string(json, i)?;
                let k = &json[k_start..(k_end - 1)];

                i = skip_ws(json, k_end);
                if json.get(i).copied() != Some(b':') {
                    return Err(ParseError::InvalidFormat);
                }
                i += 1;
                let v_start = skip_ws(json, i);

                if k == b"avg_amount" {
                    customer_avg_amount = read_double_at(json, v_start)?;
                    has_customer_avg_amount = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"tx_count_24h" {
                    let v = read_int_at(json, v_start)?;
                    out[8] = quantize(v as f64 * INV_20);
                    has_tx_count_24h = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"known_merchants" {
                    known_count = read_known_merchants_at(json, v_start, &mut known_hashes)?;
                    has_known_merchants = true;
                    i = skip_value(json, v_start)?;
                } else {
                    i = skip_value(json, v_start)?;
                }

                i = skip_ws(json, i);
                if json.get(i).copied() == Some(b',') {
                    i += 1;
                }
            }
        } else if key == b"merchant" {
            i = skip_ws(json, value_start);
            if json.get(i).copied() != Some(b'{') {
                return Err(ParseError::InvalidFormat);
            }
            i += 1;
            loop {
                i = skip_ws(json, i);
                if json.get(i).copied() == Some(b'}') {
                    i += 1;
                    break;
                }
                if json.get(i).copied() != Some(b'"') {
                    return Err(ParseError::InvalidFormat);
                }
                let k_start = i + 1;
                let k_end = skip_string(json, i)?;
                let k = &json[k_start..(k_end - 1)];

                i = skip_ws(json, k_end);
                if json.get(i).copied() != Some(b':') {
                    return Err(ParseError::InvalidFormat);
                }
                i += 1;
                let v_start = skip_ws(json, i);

                if k == b"id" {
                    let s = read_string_at(json, v_start)?;
                    merchant_hash = hash_bytes(s);
                    has_merchant_id = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"mcc" {
                    let s = read_string_at(json, v_start)?;
                    out[12] = quantize(mcc_risk(parse_mcc(s)));
                    has_mcc = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"avg_amount" {
                    let v = read_double_at(json, v_start)?;
                    out[13] = quantize(v * INV_10_000);
                    has_merchant_avg_amount = true;
                    i = skip_value(json, v_start)?;
                } else {
                    i = skip_value(json, v_start)?;
                }

                i = skip_ws(json, i);
                if json.get(i).copied() == Some(b',') {
                    i += 1;
                }
            }
        } else if key == b"terminal" {
            i = skip_ws(json, value_start);
            if json.get(i).copied() != Some(b'{') {
                return Err(ParseError::InvalidFormat);
            }
            i += 1;
            loop {
                i = skip_ws(json, i);
                if json.get(i).copied() == Some(b'}') {
                    i += 1;
                    break;
                }
                if json.get(i).copied() != Some(b'"') {
                    return Err(ParseError::InvalidFormat);
                }
                let k_start = i + 1;
                let k_end = skip_string(json, i)?;
                let k = &json[k_start..(k_end - 1)];

                i = skip_ws(json, k_end);
                if json.get(i).copied() != Some(b':') {
                    return Err(ParseError::InvalidFormat);
                }
                i += 1;
                let v_start = skip_ws(json, i);

                if k == b"is_online" {
                    let v = read_bool_at(json, v_start)?;
                    out[9] = if v { SCALE } else { 0 };
                    has_is_online = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"card_present" {
                    let v = read_bool_at(json, v_start)?;
                    out[10] = if v { SCALE } else { 0 };
                    has_card_present = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"km_from_home" {
                    let v = read_double_at(json, v_start)?;
                    out[7] = quantize(v * INV_1_000);
                    has_km_from_home = true;
                    i = skip_value(json, v_start)?;
                } else {
                    i = skip_value(json, v_start)?;
                }

                i = skip_ws(json, i);
                if json.get(i).copied() == Some(b',') {
                    i += 1;
                }
            }
        } else if key == b"last_transaction" {
            has_last_transaction = true;
            let c = json
                .get(value_start)
                .copied()
                .ok_or(ParseError::InvalidFormat)?;
            if c == b'n' {
                i = skip_value(json, value_start)?;
            } else if c == b'{' {
                i = skip_ws(json, value_start);
                i += 1;
                loop {
                    i = skip_ws(json, i);
                    if json.get(i).copied() == Some(b'}') {
                        i += 1;
                        break;
                    }
                    if json.get(i).copied() != Some(b'"') {
                        return Err(ParseError::InvalidFormat);
                    }
                    let k_start = i + 1;
                    let k_end = skip_string(json, i)?;
                    let k = &json[k_start..(k_end - 1)];

                    i = skip_ws(json, k_end);
                    if json.get(i).copied() != Some(b':') {
                        return Err(ParseError::InvalidFormat);
                    }
                    i += 1;
                    let v_start = skip_ws(json, i);

                    if k == b"timestamp" {
                        let s = read_string_at(json, v_start)?;
                        let parsed = parse_datetime(s)?;
                        last_timestamp = Some(parsed.epoch_minute);
                        i = skip_value(json, v_start)?;
                    } else if k == b"km_from_current" {
                        let v = read_double_at(json, v_start)?;
                        last_km_from_current = Some(v);
                        i = skip_value(json, v_start)?;
                    } else {
                        i = skip_value(json, v_start)?;
                    }

                    i = skip_ws(json, i);
                    if json.get(i).copied() == Some(b',') {
                        i += 1;
                    }
                }
            } else {
                return Err(ParseError::InvalidFormat);
            }
        } else {
            i = skip_value(json, value_start)?;
        }

        i = skip_ws(json, i);
        if json.get(i).copied() == Some(b',') {
            i += 1;
        }
    }

    if !has_amount {
        return Err(ParseError::MissingField);
    }
    if !has_installments {
        return Err(ParseError::MissingField);
    }
    if !has_requested_at {
        return Err(ParseError::MissingField);
    }
    if !has_customer_avg_amount {
        return Err(ParseError::MissingField);
    }
    if !has_tx_count_24h {
        return Err(ParseError::MissingField);
    }
    if !has_known_merchants {
        return Err(ParseError::MissingField);
    }
    if !has_merchant_id {
        return Err(ParseError::MissingField);
    }
    if !has_mcc {
        return Err(ParseError::MissingField);
    }
    if !has_merchant_avg_amount {
        return Err(ParseError::MissingField);
    }
    if !has_is_online {
        return Err(ParseError::MissingField);
    }
    if !has_card_present {
        return Err(ParseError::MissingField);
    }
    if !has_km_from_home {
        return Err(ParseError::MissingField);
    }
    if !has_last_transaction {
        return Err(ParseError::MissingField);
    }

    if last_timestamp.is_some() || last_km_from_current.is_some() {
        if last_timestamp.is_none() || last_km_from_current.is_none() {
            return Err(ParseError::InvalidFormat);
        }
    }

    out[2] = if customer_avg_amount > 0.0 {
        quantize((amount / customer_avg_amount) / 10.0)
    } else {
        SCALE
    };

    let known = known_hashes[..known_count].contains(&merchant_hash);
    out[11] = if known { 0 } else { SCALE };

    if let (Some(lm), Some(lk)) = (last_timestamp, last_km_from_current) {
        let minutes_diff = requested_minute.saturating_sub(lm);
        out[5] = quantize(minutes_diff as f64 * INV_1_440);
        out[6] = quantize(lk * INV_1_000);
    } else {
        out[5] = -SCALE;
        out[6] = -SCALE;
    }

    Ok(())
}

fn read_known_merchants_at(
    json: &[u8],
    start: usize,
    hashes: &mut [u64; 64],
) -> Result<usize, ParseError> {
    if start >= json.len() || json[start] != b'[' {
        return Err(ParseError::InvalidFormat);
    }
    let rel_end = json[start..]
        .iter()
        .position(|&b| b == b']')
        .ok_or(ParseError::InvalidFormat)?;
    let array_end = start + rel_end;
    let mut i = start + 1;
    let mut count = 0usize;
    while i < array_end {
        while i < array_end && json[i] != b'"' {
            i += 1;
        }
        if i >= array_end {
            break;
        }
        let content_start = i + 1;
        let rel = json[content_start..array_end]
            .iter()
            .position(|&b| b == b'"')
            .ok_or(ParseError::InvalidFormat)?;
        if count < hashes.len() {
            hashes[count] = hash_bytes(&json[content_start..content_start + rel]);
            count += 1;
        }
        i = content_start + rel + 1;
    }
    Ok(count)
}

struct Payload<'a> {
    transaction: Transaction<'a>,
    customer: Customer<'a>,
    merchant: Merchant<'a>,
    terminal: Terminal,
    last_transaction: Option<LastTransaction<'a>>,
}

struct Transaction<'a> {
    amount: f64,
    installments: i32,
    requested_at: Cow<'a, str>,
}

struct Customer<'a> {
    avg_amount: f64,
    tx_count_24h: i32,
    known_merchants: Vec<Cow<'a, str>>,
}

struct Merchant<'a> {
    id: Cow<'a, str>,
    mcc: Cow<'a, str>,
    avg_amount: f64,
}

struct Terminal {
    is_online: bool,
    card_present: bool,
    km_from_home: f64,
}

struct LastTransaction<'a> {
    timestamp: Cow<'a, str>,
    km_from_current: f64,
}

impl<'de: 'a, 'a> Deserialize<'de> for Payload<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PayloadVisitor;

        impl<'de> Visitor<'de> for PayloadVisitor {
            type Value = Payload<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("fraud score payload object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut transaction = None;
                let mut customer = None;
                let mut merchant = None;
                let mut terminal = None;
                let mut last_transaction = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "transaction") {
                        transaction = Some(map.next_value()?);
                    } else if field_matches(&key, "customer") {
                        customer = Some(map.next_value()?);
                    } else if field_matches(&key, "merchant") {
                        merchant = Some(map.next_value()?);
                    } else if field_matches(&key, "terminal") {
                        terminal = Some(map.next_value()?);
                    } else if field_matches(&key, "last_transaction") {
                        last_transaction = map.next_value()?;
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(Payload {
                    transaction: transaction
                        .ok_or_else(|| de::Error::missing_field("transaction"))?,
                    customer: customer.ok_or_else(|| de::Error::missing_field("customer"))?,
                    merchant: merchant.ok_or_else(|| de::Error::missing_field("merchant"))?,
                    terminal: terminal.ok_or_else(|| de::Error::missing_field("terminal"))?,
                    last_transaction,
                })
            }
        }

        deserializer.deserialize_map(PayloadVisitor)
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Transaction<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TransactionVisitor;

        impl<'de> Visitor<'de> for TransactionVisitor {
            type Value = Transaction<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("transaction object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut amount = None;
                let mut installments = None;
                let mut requested_at = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "amount") {
                        amount = Some(map.next_value()?);
                    } else if field_matches(&key, "installments") {
                        installments = Some(map.next_value()?);
                    } else if field_matches(&key, "requested_at") {
                        requested_at = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(Transaction {
                    amount: amount.ok_or_else(|| de::Error::missing_field("amount"))?,
                    installments: installments
                        .ok_or_else(|| de::Error::missing_field("installments"))?,
                    requested_at: requested_at
                        .ok_or_else(|| de::Error::missing_field("requested_at"))?,
                })
            }
        }

        deserializer.deserialize_map(TransactionVisitor)
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Customer<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CustomerVisitor;

        impl<'de> Visitor<'de> for CustomerVisitor {
            type Value = Customer<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("customer object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut avg_amount = None;
                let mut tx_count_24h = None;
                let mut known_merchants = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "avg_amount") {
                        avg_amount = Some(map.next_value()?);
                    } else if field_matches(&key, "tx_count_24h") {
                        tx_count_24h = Some(map.next_value()?);
                    } else if field_matches(&key, "known_merchants") {
                        known_merchants = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(Customer {
                    avg_amount: avg_amount.ok_or_else(|| de::Error::missing_field("avg_amount"))?,
                    tx_count_24h: tx_count_24h
                        .ok_or_else(|| de::Error::missing_field("tx_count_24h"))?,
                    known_merchants: known_merchants
                        .ok_or_else(|| de::Error::missing_field("known_merchants"))?,
                })
            }
        }

        deserializer.deserialize_map(CustomerVisitor)
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Merchant<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MerchantVisitor;

        impl<'de> Visitor<'de> for MerchantVisitor {
            type Value = Merchant<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("merchant object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut id = None;
                let mut mcc = None;
                let mut avg_amount = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "id") {
                        id = Some(map.next_value()?);
                    } else if field_matches(&key, "mcc") {
                        mcc = Some(map.next_value()?);
                    } else if field_matches(&key, "avg_amount") {
                        avg_amount = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(Merchant {
                    id: id.ok_or_else(|| de::Error::missing_field("id"))?,
                    mcc: mcc.ok_or_else(|| de::Error::missing_field("mcc"))?,
                    avg_amount: avg_amount.ok_or_else(|| de::Error::missing_field("avg_amount"))?,
                })
            }
        }

        deserializer.deserialize_map(MerchantVisitor)
    }
}

impl<'de> Deserialize<'de> for Terminal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TerminalVisitor;

        impl<'de> Visitor<'de> for TerminalVisitor {
            type Value = Terminal;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("terminal object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut is_online = None;
                let mut card_present = None;
                let mut km_from_home = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "is_online") {
                        is_online = Some(map.next_value()?);
                    } else if field_matches(&key, "card_present") {
                        card_present = Some(map.next_value()?);
                    } else if field_matches(&key, "km_from_home") {
                        km_from_home = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(Terminal {
                    is_online: is_online.ok_or_else(|| de::Error::missing_field("is_online"))?,
                    card_present: card_present
                        .ok_or_else(|| de::Error::missing_field("card_present"))?,
                    km_from_home: km_from_home
                        .ok_or_else(|| de::Error::missing_field("km_from_home"))?,
                })
            }
        }

        deserializer.deserialize_map(TerminalVisitor)
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for LastTransaction<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct LastTransactionVisitor;

        impl<'de> Visitor<'de> for LastTransactionVisitor {
            type Value = LastTransaction<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("last transaction object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut timestamp = None;
                let mut km_from_current = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "timestamp") {
                        timestamp = Some(map.next_value()?);
                    } else if field_matches(&key, "km_from_current") {
                        km_from_current = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(LastTransaction {
                    timestamp: timestamp.ok_or_else(|| de::Error::missing_field("timestamp"))?,
                    km_from_current: km_from_current
                        .ok_or_else(|| de::Error::missing_field("km_from_current"))?,
                })
            }
        }

        deserializer.deserialize_map(LastTransactionVisitor)
    }
}

fn field_matches(actual: &str, expected: &str) -> bool {
    let mut actual = actual.bytes().filter(|&b| b != b'_' && b != b'-');
    let mut expected = expected.bytes().filter(|&b| b != b'_' && b != b'-');

    loop {
        match (actual.next(), expected.next()) {
            (Some(a), Some(e)) if a.eq_ignore_ascii_case(&e) => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

fn try_parse_serde(payload: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
    let parsed: Payload = serde_json::from_slice(payload).map_err(|_| ParseError::InvalidFormat)?;

    let requested_parsed = parse_datetime(parsed.transaction.requested_at.as_bytes())?;
    let requested_minute = requested_parsed.epoch_minute;

    out[0] = quantize(parsed.transaction.amount * INV_10_000);
    out[1] = quantize(parsed.transaction.installments as f64 * INV_12);
    out[3] = quantize(requested_parsed.hour as f64 * INV_23);
    out[4] = quantize(requested_parsed.day_of_week as f64 * INV_6);
    out[7] = quantize(parsed.terminal.km_from_home * INV_1_000);
    out[8] = quantize(parsed.customer.tx_count_24h as f64 * INV_20);
    out[9] = if parsed.terminal.is_online { SCALE } else { 0 };
    out[10] = if parsed.terminal.card_present {
        SCALE
    } else {
        0
    };
    out[12] = quantize(mcc_risk(parse_mcc(parsed.merchant.mcc.as_bytes())));
    out[13] = quantize(parsed.merchant.avg_amount * INV_10_000);

    if let Some(last_transaction) = parsed.last_transaction {
        let last_parsed = parse_datetime(last_transaction.timestamp.as_bytes())?;
        let last_minute = last_parsed.epoch_minute;
        let minutes_diff = requested_minute.saturating_sub(last_minute);
        out[5] = quantize(minutes_diff as f64 * INV_1_440);
        out[6] = quantize(last_transaction.km_from_current * INV_1_000);
    } else {
        out[5] = -SCALE;
        out[6] = -SCALE;
    }

    let mut known_hashes = [0u64; 64];
    let known_count = std::cmp::min(parsed.customer.known_merchants.len(), 64);
    for i in 0..known_count {
        known_hashes[i] = hash_bytes(parsed.customer.known_merchants[i].as_bytes());
    }

    finish_vector(
        out,
        parsed.transaction.amount,
        parsed.customer.avg_amount,
        hash_bytes(parsed.merchant.id.as_bytes()),
        &known_hashes[..known_count],
    );
    Ok(())
}

fn finish_vector(
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

fn fast_parse_f64(s: &[u8]) -> Option<f64> {
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

fn read_double_at(json: &[u8], start: usize) -> Result<f64, ParseError> {
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
        return Err(ParseError::InvalidValue);
    }
    let token = &s[..end];
    if let Some(val) = fast_parse_f64(token) {
        Ok(val)
    } else {
        let text = std::str::from_utf8(token).map_err(|_| ParseError::InvalidValue)?;
        text.parse::<f64>().map_err(|_| ParseError::InvalidValue)
    }
}

fn read_int_at(json: &[u8], start: usize) -> Result<i32, ParseError> {
    let s = &json[start..];
    let mut end = 0usize;
    if s.first().copied() == Some(b'-') {
        end += 1;
    }
    while s.get(end).map_or(false, |&b| b.is_ascii_digit()) {
        end += 1;
    }
    if end == 0 || (end == 1 && s[0] == b'-') {
        return Err(ParseError::InvalidValue);
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

fn read_bool_at(json: &[u8], start: usize) -> Result<bool, ParseError> {
    let s = &json[start..];
    if s.starts_with(b"true") {
        Ok(true)
    } else if s.starts_with(b"false") {
        Ok(false)
    } else {
        Err(ParseError::InvalidValue)
    }
}

fn read_string_at<'a>(json: &'a [u8], start: usize) -> Result<&'a [u8], ParseError> {
    if start >= json.len() || json[start] != b'"' {
        return Err(ParseError::InvalidValue);
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
    Err(ParseError::InvalidValue)
}

fn is_json_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\r' | b'\t')
}

#[inline(always)]
fn hash_bytes(value: &[u8]) -> u64 {
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

fn parse_mcc(mcc: &[u8]) -> i32 {
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

fn mcc_risk(mcc: i32) -> f64 {
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
struct ParsedDateTime {
    epoch_minute: i64,
    hour: i32,
    day_of_week: i32,
}

fn parse_datetime(iso: &[u8]) -> Result<ParsedDateTime, ParseError> {
    if iso.len() < 16 {
        return Err(ParseError::InvalidValue);
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

fn parse2(s: &[u8], offset: usize) -> Result<i32, ParseError> {
    if offset + 2 > s.len() {
        return Err(ParseError::InvalidValue);
    }
    let a = s[offset].wrapping_sub(b'0');
    let b = s[offset + 1].wrapping_sub(b'0');
    if a > 9 || b > 9 {
        return Err(ParseError::InvalidValue);
    }
    Ok((a as i32) * 10 + (b as i32))
}

fn parse4(s: &[u8], offset: usize) -> Result<i32, ParseError> {
    if offset + 4 > s.len() {
        return Err(ParseError::InvalidValue);
    }
    let a = s[offset].wrapping_sub(b'0');
    let b = s[offset + 1].wrapping_sub(b'0');
    let c = s[offset + 2].wrapping_sub(b'0');
    let d = s[offset + 3].wrapping_sub(b'0');
    if a > 9 || b > 9 || c > 9 || d > 9 {
        return Err(ParseError::InvalidValue);
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

#[cfg(test)]
mod tests;
