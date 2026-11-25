//! Async Curious read SDK and local decryption.

mod client;
mod decryption;
#[rustfmt::skip]
#[allow(dead_code, clippy::all)]
mod generated;

pub use chrono::NaiveDate;
pub use client::{
    Applet, AppletPage, Client, DEFAULT_PAGE_SIZE, Error, ExportPage, Login, MfaChallenge, Paging,
};
pub use decryption::{
    AnswerOutcome, DecryptedData, EncryptedData, FieldOutcome, decrypt, encrypted_data,
};
pub use uuid::Uuid;
