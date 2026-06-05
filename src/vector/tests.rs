#[cfg(test)]
mod tests {
    use super::super::*;

    const BASELINE: &[u8] = br#"{"id":"tx-1","transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z"},"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.70},"last_transaction":{"timestamp":"2026-03-11T14:58:35Z","km_from_current":18.86}}"#;

    #[test]
    fn parses_required_payload() {
        let query = parse_vec(BASELINE);

        assert_eq!(query[0], 385);
        assert_eq!(query[1], 2500);
        assert_eq!(query[5], 2257);
        assert_eq!(query[6], 189);
        assert_eq!(query[7], 137);
        assert_eq!(query[8], 1500);
        assert_eq!(query[9], 0);
        assert_eq!(query[10], 10000);
        assert_eq!(query[13], 299);
    }

    #[test]
    fn object_order_does_not_change_vector() {
        let customer_first = br#"{"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"id":"tx-1","last_transaction":{"timestamp":"2026-03-11T14:58:35Z","km_from_current":18.8626479774},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.7090520965},"transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z"}}"#;

        assert_eq!(parse_vec(customer_first), parse_vec(BASELINE));
    }

    #[test]
    fn single_pass_matches_serde_fallback() {
        let mut fast = [0i16; 16];
        let mut fallback = [0i16; 16];

        try_parse_single_pass(BASELINE, &mut fast).expect("single-pass parse failed");
        try_parse_serde(BASELINE, &mut fallback).expect("serde parse failed");

        assert_eq!(fast, fallback);
    }

    #[test]
    fn compact_ordered_matches_serde_fallback() {
        let mut fast = [0i16; 16];
        let mut fallback = [0i16; 16];

        try_parse_compact_ordered(BASELINE, &mut fast).expect("compact parse failed");
        try_parse_serde(BASELINE, &mut fallback).expect("serde parse failed");

        assert_eq!(fast, fallback);
    }

    #[test]
    fn compact_ordered_accepts_null_last_transaction() {
        let payload = br#"{"id":"tx-1","transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z"},"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.70},"last_transaction":null}"#;
        let mut fast = [0i16; 16];
        let mut fallback = [0i16; 16];

        try_parse_compact_ordered(payload, &mut fast).expect("compact parse failed");
        try_parse_serde(payload, &mut fallback).expect("serde parse failed");

        assert_eq!(fast, fallback);
        assert_eq!(fast[5], -SCALE);
        assert_eq!(fast[6], -SCALE);
    }

    #[test]
    fn fallback_accepts_case_and_separator_variants() {
        let variant = br#"{"ID":"tx-1","TRANSACTION":{"AMOUNT":384.88,"installments":3,"requestedAt":"2026-03-11T20:23:35Z"},"CUSTOMER":{"avg-amount":769.76,"txCount24h":3,"knownMerchants":["MERC-009","MERC-001"]},"MERCHANT":{"ID":"MERC-001","MCC":"5912","AVG-AMOUNT":298.95},"TERMINAL":{"isOnline":false,"card-present":true,"kmFromHome":13.70},"lastTransaction":{"TIMESTAMP":"2026-03-11T14:58:35Z","kmFromCurrent":18.86}}"#;

        assert_eq!(parse_vec(variant), parse_vec(BASELINE));
    }

    #[test]
    fn invalid_payloads_are_rejected() {
        let invalid_payloads: &[&[u8]] = &[
            b"",
            b"{}",
            br#"{"transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z"},"customer":{"tx_count_24h":3,"known_merchants":["MERC-009"]},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.70},"last_transaction":null}"#,
            br#"{"id":"tx-1","transaction":{"amount":"384.88","installments":3,"requested_at":"2026-03-11T20:23:35Z"},"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.7090520965},"last_transaction":null}"#,
        ];

        for payload in invalid_payloads {
            let mut query = [0i16; 16];
            assert!(
                parse_query(payload, &mut query).is_err(),
                "payload should be rejected: {}",
                String::from_utf8_lossy(payload)
            );
        }
    }

    fn parse_vec(payload: &[u8]) -> [i16; 16] {
        let mut query = [0i16; 16];
        parse_query(payload, &mut query).expect("payload should parse");
        query
    }
}
