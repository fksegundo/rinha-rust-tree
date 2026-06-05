use crate::QueryVector;

use super::ParseError;
use super::helpers::{finish_vector, hash_bytes, mcc_risk, parse_datetime, parse_mcc, quantize};

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, Visitor};
use std::borrow::Cow;
use std::fmt;

const INV_10_000: f64 = 1.0 / 10_000.0;
const INV_12: f64 = 1.0 / 12.0;
const INV_23: f64 = 1.0 / 23.0;
const INV_6: f64 = 1.0 / 6.0;
const INV_1_440: f64 = 1.0 / 1_440.0;
const INV_1_000: f64 = 1.0 / 1_000.0;
const INV_20: f64 = 1.0 / 20.0;

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

pub fn try_parse_serde(payload: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
    let parsed: Payload = serde_json::from_slice(payload).map_err(|_| ParseError::InvalidFormat)?;

    let requested_parsed = parse_datetime(parsed.transaction.requested_at.as_bytes())?;
    let requested_minute = requested_parsed.epoch_minute;

    out[0] = quantize(parsed.transaction.amount * INV_10_000);
    out[1] = quantize(parsed.transaction.installments as f64 * INV_12);
    out[3] = quantize(requested_parsed.hour as f64 * INV_23);
    out[4] = quantize(requested_parsed.day_of_week as f64 * INV_6);
    out[7] = quantize(parsed.terminal.km_from_home * INV_1_000);
    out[8] = quantize(parsed.customer.tx_count_24h as f64 * INV_20);
    out[9] = if parsed.terminal.is_online {
        crate::SCALE
    } else {
        0
    };
    out[10] = if parsed.terminal.card_present {
        crate::SCALE
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
        out[5] = -crate::SCALE;
        out[6] = -crate::SCALE;
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
