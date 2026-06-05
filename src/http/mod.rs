mod parser;
mod responses;

pub use parser::BUF_SIZE;
pub use parser::{BufferStep, Method, Request, parse_request, process_one_request};
pub use responses::{
    FRAUD_RESPONSES, RESPONSE_BAD_REQUEST, RESPONSE_FRAUD_0, RESPONSE_FRAUD_1, RESPONSE_FRAUD_2,
    RESPONSE_FRAUD_3, RESPONSE_FRAUD_4, RESPONSE_FRAUD_5, RESPONSE_NOT_FOUND, RESPONSE_NOT_READY,
    RESPONSE_READY,
};
