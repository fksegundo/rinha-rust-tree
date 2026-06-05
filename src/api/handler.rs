use crate::http;
use crate::index::SpecialistIndex;
use crate::vector;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn handle_request(
    req: &http::Request,
    index: &SpecialistIndex,
    ready: &AtomicBool,
) -> &'static [u8] {
    match req.method {
        http::Method::Get if req.path == b"/ready" => {
            if is_ready(ready) {
                http::RESPONSE_READY
            } else {
                http::RESPONSE_NOT_READY
            }
        }
        http::Method::Post if req.path == b"/fraud-score" => {
            if !can_serve_fraud(ready) {
                return http::RESPONSE_NOT_READY;
            }
            let mut query = [0i16; 16];
            match vector::parse_query(req.body, &mut query) {
                Ok(()) => {
                    let count = index.predict_fraud_count(&query) as usize;
                    if count < http::FRAUD_RESPONSES.len() {
                        http::FRAUD_RESPONSES[count]
                    } else {
                        http::FRAUD_RESPONSES[5]
                    }
                }
                Err(_) => http::RESPONSE_BAD_REQUEST,
            }
        }
        _ => http::RESPONSE_NOT_FOUND,
    }
}

fn is_ready(ready: &AtomicBool) -> bool {
    ready.load(Ordering::Acquire)
}

fn can_serve_fraud(ready: &AtomicBool) -> bool {
    is_ready(ready) || super::server::accept_warmup()
}
