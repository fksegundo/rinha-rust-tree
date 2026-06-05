use super::responses;
use std::sync::OnceLock;

pub const MAX_BODY_LEN: usize = 8192;
pub const BUF_SIZE: usize = 2048;

#[derive(Debug, PartialEq, Eq)]
pub enum BufferStep {
    Respond {
        consumed: usize,
        response: &'static [u8],
        keep_alive: bool,
    },
    RejectAndClose {
        response: &'static [u8],
    },
    NeedMore,
}

pub fn process_one_request<F>(buf: &[u8], mut handler: F) -> BufferStep
where
    F: FnMut(&Request) -> &'static [u8],
{
    match parse_request_result(buf) {
        ParseResult::Complete(req, consumed) => {
            let response = handler(&req);
            BufferStep::Respond {
                consumed,
                response,
                keep_alive: req.keep_alive,
            }
        }
        ParseResult::Reject(response) => BufferStep::RejectAndClose { response },
        ParseResult::NeedMore => BufferStep::NeedMore,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

pub struct Request<'a> {
    pub method: Method,
    pub path: &'a [u8],
    pub body: &'a [u8],
    pub keep_alive: bool,
}

pub fn parse_request(buf: &[u8]) -> Option<(Request<'_>, usize)> {
    match parse_request_result(buf) {
        ParseResult::Complete(req, consumed) => Some((req, consumed)),
        ParseResult::NeedMore | ParseResult::Reject(_) => None,
    }
}

pub(crate) enum ParseResult<'a> {
    Complete(Request<'a>, usize),
    Reject(&'static [u8]),
    NeedMore,
}

pub(crate) fn parse_request_result(buf: &[u8]) -> ParseResult<'_> {
    let header_end = match find_header_end(buf) {
        Some(header_end) => header_end,
        None => return ParseResult::NeedMore,
    };
    let (method, path, headers_len) = match parse_first_line(buf) {
        Some(parts) => parts,
        None => return ParseResult::Reject(responses::RESPONSE_BAD_REQUEST),
    };
    let content_length = find_content_length(&buf[headers_len..header_end]);

    let path_bytes = &buf[path.0..path.1];
    if let Some(response) = early_rejection(method, path_bytes, content_length) {
        return ParseResult::Reject(response);
    }

    let body_start = header_end;
    let body_end = body_start + content_length;
    if buf.len() < body_end {
        return ParseResult::NeedMore;
    }

    let keep_alive = assume_keep_alive() || !contains_connection_close(&buf[..header_end]);

    ParseResult::Complete(
        Request {
            method,
            path: &buf[path.0..path.1],
            body: &buf[body_start..body_end],
            keep_alive,
        },
        body_end,
    )
}

fn early_rejection(method: Method, path: &[u8], content_length: usize) -> Option<&'static [u8]> {
    match (method, path) {
        (Method::Get, b"/ready") => None,
        (Method::Post, b"/fraud-score") if content_length <= MAX_BODY_LEN => None,
        (Method::Post, b"/fraud-score") => Some(responses::RESPONSE_BAD_REQUEST),
        _ => Some(responses::RESPONSE_NOT_FOUND),
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    const NEEDLE: &[u8] = b"\r\n\r\n";
    let found = unsafe {
        libc::memmem(
            buf.as_ptr().cast(),
            buf.len(),
            NEEDLE.as_ptr().cast(),
            NEEDLE.len(),
        )
    };
    if found.is_null() {
        return None;
    }
    Some((found as usize) - (buf.as_ptr() as usize) + NEEDLE.len())
}

fn parse_first_line(buf: &[u8]) -> Option<(Method, (usize, usize), usize)> {
    let end = buf.iter().position(|&b| b == b'\r')?;
    let method_end = buf[..end].iter().position(|&b| b == b' ')?;
    let method = if &buf[..method_end] == b"GET" {
        Method::Get
    } else if &buf[..method_end] == b"POST" {
        Method::Post
    } else {
        return None;
    };

    let path_start = method_end + 1;
    if path_start >= end {
        return None;
    }
    let rel_path_end = buf[path_start..end].iter().position(|&b| b == b' ')?;
    let path_end = path_start + rel_path_end;
    if path_end == path_start {
        return None;
    }
    Some((method, (path_start, path_end), end + 2))
}

fn find_content_length(headers: &[u8]) -> usize {
    const EXACT_NEEDLE: &[u8] = b"Content-Length:";
    const NEEDLE: &[u8] = b"content-length:";
    if let Some(pos) = find_bytes(headers, EXACT_NEEDLE) {
        return parse_content_length_value(&headers[pos + EXACT_NEEDLE.len()..]);
    }

    let n = headers.len();
    if n < NEEDLE.len() {
        return 0;
    }
    let mut i = 0;
    while i + NEEDLE.len() <= n {
        if headers[i].to_ascii_lowercase() == b'c' {
            let window = &headers[i..i + NEEDLE.len()];
            if window.eq_ignore_ascii_case(NEEDLE) {
                return parse_content_length_value(&headers[i + NEEDLE.len()..]);
            }
        }
        i += 1;
    }
    0
}

fn parse_content_length_value(rest: &[u8]) -> usize {
    let val_start = rest.iter().position(|&b| !is_ws(b)).unwrap_or(0);
    let val_end = rest[val_start..]
        .iter()
        .position(|&b| b == b'\r' || is_ws(b))
        .unwrap_or(rest.len() - val_start);
    let mut num = 0usize;
    for &b in &rest[val_start..val_start + val_end] {
        if !b.is_ascii_digit() {
            return 0;
        }
        num = num.saturating_mul(10).saturating_add((b - b'0') as usize);
    }
    num
}

fn contains_connection_close(headers: &[u8]) -> bool {
    if find_bytes(headers, b"Connection: close").is_some()
        || find_bytes(headers, b"connection: close").is_some()
    {
        return true;
    }
    headers
        .windows(17)
        .any(|w| w.eq_ignore_ascii_case(b"Connection: close"))
}

fn assume_keep_alive() -> bool {
    static ASSUME_KEEP_ALIVE: OnceLock<bool> = OnceLock::new();
    *ASSUME_KEEP_ALIVE.get_or_init(|| {
        std::env::var("RINHA_ASSUME_KEEP_ALIVE")
            .map(|value| value != "0")
            .unwrap_or(false)
    })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let found = unsafe {
        libc::memmem(
            haystack.as_ptr().cast(),
            haystack.len(),
            needle.as_ptr().cast(),
            needle.len(),
        )
    };
    if found.is_null() {
        None
    } else {
        Some((found as usize) - (haystack.as_ptr() as usize))
    }
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_first_line_without_allocating_parts() {
        let request =
            b"POST /fraud-score HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}";
        let (parsed, consumed) = parse_request(request).expect("request should parse");

        assert_eq!(parsed.method, Method::Post);
        assert_eq!(parsed.path, b"/fraud-score");
        assert_eq!(parsed.body, b"{}");
        assert_eq!(consumed, request.len());
    }

    #[test]
    fn parses_content_length_digits_directly() {
        assert_eq!(
            find_content_length(b"Host: x\r\nContent-Length: 123\r\n"),
            123
        );
        assert_eq!(find_content_length(b"content-length:\t42\r\n"), 42);
        assert_eq!(find_content_length(b"Content-Length: nope\r\n"), 0);
    }

    #[test]
    fn rejects_unknown_path_after_headers_without_waiting_for_body() {
        let request = b"POST /missing HTTP/1.1\r\nHost: localhost\r\nContent-Length: 64000\r\n\r\n";

        match parse_request_result(request) {
            ParseResult::Reject(response) => {
                assert_eq!(response, responses::RESPONSE_NOT_FOUND);
            }
            _ => panic!("unknown path should be rejected after headers"),
        }
    }

    #[test]
    fn rejects_oversized_fraud_body_after_headers_without_waiting_for_body() {
        let request =
            b"POST /fraud-score HTTP/1.1\r\nHost: localhost\r\nContent-Length: 64000\r\n\r\n";

        match parse_request_result(request) {
            ParseResult::Reject(response) => {
                assert_eq!(response, responses::RESPONSE_BAD_REQUEST);
            }
            _ => panic!("oversized fraud body should be rejected after headers"),
        }
    }

    #[test]
    fn keep_alive_processes_two_requests_in_one_buffer() {
        let req1 = b"GET /ready HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let req2 = b"GET /ready HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(req1);
        buf.extend_from_slice(req2);

        let mut offset = 0usize;
        for _ in 0..2 {
            match process_one_request(&buf[offset..], |_| responses::RESPONSE_READY) {
                BufferStep::Respond {
                    consumed,
                    response,
                    keep_alive,
                } => {
                    assert_eq!(response, responses::RESPONSE_READY);
                    assert!(keep_alive);
                    offset += consumed;
                }
                other => panic!("unexpected step: {:?}", other),
            }
        }
        assert_eq!(offset, buf.len());
    }

    #[test]
    fn pipeline_returns_two_responses_from_one_buffer() {
        let body = b"{}";
        let req1 = format!(
            "POST /fraud-score HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut buf = Vec::new();
        buf.extend_from_slice(req1.as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"GET /ready HTTP/1.1\r\nHost: localhost\r\n\r\n");

        let mut responses_vec = Vec::new();
        let mut offset = 0usize;
        while offset < buf.len() {
            match process_one_request(&buf[offset..], |req| {
                if req.path == b"/ready" {
                    responses::RESPONSE_READY
                } else {
                    responses::RESPONSE_FRAUD_0
                }
            }) {
                BufferStep::Respond {
                    consumed, response, ..
                } => {
                    responses_vec.push(response);
                    offset += consumed;
                }
                other => panic!("unexpected step: {:?}", other),
            }
        }
        assert_eq!(responses_vec.len(), 2);
        assert_eq!(responses_vec[0], responses::RESPONSE_FRAUD_0);
        assert_eq!(responses_vec[1], responses::RESPONSE_READY);
    }
}
