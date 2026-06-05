pub const RESPONSE_READY: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
pub const RESPONSE_NOT_READY: &[u8] =
    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n";
pub const RESPONSE_FRAUD_0: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 35\r\n\r\n{\"approved\":true,\"fraud_score\":0.0}";
pub const RESPONSE_FRAUD_1: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 35\r\n\r\n{\"approved\":true,\"fraud_score\":0.2}";
pub const RESPONSE_FRAUD_2: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 35\r\n\r\n{\"approved\":true,\"fraud_score\":0.4}";
pub const RESPONSE_FRAUD_3: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 36\r\n\r\n{\"approved\":false,\"fraud_score\":0.6}";
pub const RESPONSE_FRAUD_4: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 36\r\n\r\n{\"approved\":false,\"fraud_score\":0.8}";
pub const RESPONSE_FRAUD_5: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 36\r\n\r\n{\"approved\":false,\"fraud_score\":1.0}";
pub const RESPONSE_BAD_REQUEST: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
pub const RESPONSE_NOT_FOUND: &[u8] =
    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

pub const FRAUD_RESPONSES: [&[u8]; 6] = [
    RESPONSE_FRAUD_0,
    RESPONSE_FRAUD_1,
    RESPONSE_FRAUD_2,
    RESPONSE_FRAUD_3,
    RESPONSE_FRAUD_4,
    RESPONSE_FRAUD_5,
];
