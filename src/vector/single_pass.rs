use crate::QueryVector;

use super::ParseError;
use super::helpers::{hash_bytes, quantize};

const INV_10_000: f64 = 1.0 / 10_000.0;
const INV_12: f64 = 1.0 / 12.0;
const INV_23: f64 = 1.0 / 23.0;
const INV_6: f64 = 1.0 / 6.0;
const INV_1_440: f64 = 1.0 / 1_440.0;
const INV_1_000: f64 = 1.0 / 1_000.0;
const INV_20: f64 = 1.0 / 20.0;

#[inline(always)]
fn skip_ws(json: &[u8], mut i: usize) -> usize {
    while i < json.len() && super::helpers::is_json_whitespace(json[i]) {
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

pub fn try_parse_single_pass(json: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
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
                    amount = super::helpers::read_double_at(json, v_start)?;
                    out[0] = quantize(amount * INV_10_000);
                    has_amount = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"installments" {
                    let v = super::helpers::read_int_at(json, v_start)?;
                    out[1] = quantize(v as f64 * INV_12);
                    has_installments = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"requested_at" {
                    let s = super::helpers::read_string_at(json, v_start)?;
                    let parsed = super::helpers::parse_datetime(s)?;
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
                    customer_avg_amount = super::helpers::read_double_at(json, v_start)?;
                    has_customer_avg_amount = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"tx_count_24h" {
                    let v = super::helpers::read_int_at(json, v_start)?;
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
                    let s = super::helpers::read_string_at(json, v_start)?;
                    merchant_hash = hash_bytes(s);
                    has_merchant_id = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"mcc" {
                    let s = super::helpers::read_string_at(json, v_start)?;
                    out[12] = quantize(super::helpers::mcc_risk(super::helpers::parse_mcc(s)));
                    has_mcc = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"avg_amount" {
                    let v = super::helpers::read_double_at(json, v_start)?;
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
                    let v = super::helpers::read_bool_at(json, v_start)?;
                    out[9] = if v { crate::SCALE } else { 0 };
                    has_is_online = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"card_present" {
                    let v = super::helpers::read_bool_at(json, v_start)?;
                    out[10] = if v { crate::SCALE } else { 0 };
                    has_card_present = true;
                    i = skip_value(json, v_start)?;
                } else if k == b"km_from_home" {
                    let v = super::helpers::read_double_at(json, v_start)?;
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
                        let s = super::helpers::read_string_at(json, v_start)?;
                        let parsed = super::helpers::parse_datetime(s)?;
                        last_timestamp = Some(parsed.epoch_minute);
                        i = skip_value(json, v_start)?;
                    } else if k == b"km_from_current" {
                        let v = super::helpers::read_double_at(json, v_start)?;
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
        crate::SCALE
    };

    let known = known_hashes[..known_count].contains(&merchant_hash);
    out[11] = if known { 0 } else { crate::SCALE };

    if let (Some(lm), Some(lk)) = (last_timestamp, last_km_from_current) {
        let minutes_diff = requested_minute.saturating_sub(lm);
        out[5] = quantize(minutes_diff as f64 * INV_1_440);
        out[6] = quantize(lk * INV_1_000);
    } else {
        out[5] = -crate::SCALE;
        out[6] = -crate::SCALE;
    }

    Ok(())
}
