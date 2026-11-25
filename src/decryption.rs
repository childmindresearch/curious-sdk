use std::fmt;

use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
use num_bigint_dig::BigUint;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};

use crate::client::{Applet, Error, ExportPage, redacted_debug};

#[derive(Clone)]
pub struct EncryptedData {
    page: ExportPage,
    prime: Vec<u8>,
    account_id: String,
}

#[derive(Clone)]
pub struct DecryptedData {
    page: ExportPage,
    outcomes: Vec<AnswerOutcome>,
}

/// Outcomes for the answer and event fields, in the same order as the answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnswerOutcome {
    pub answer: FieldOutcome,
    pub events: FieldOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldOutcome {
    Unchanged,
    MissingOrInvalidKey,
    Failed,
    Decrypted,
}

redacted_debug!(EncryptedData, DecryptedData);

impl DecryptedData {
    pub fn answers(&self) -> &[Value] {
        self.page.answers()
    }
    pub fn activities(&self) -> &[Value] {
        self.page.activities()
    }
    pub fn outcomes(&self) -> &[AnswerOutcome] {
        &self.outcomes
    }
}

/// Combine an export page with encryption metadata from the same applet and origin.
pub fn encrypted_data(page: ExportPage, applet: &Applet) -> Result<EncryptedData, Error> {
    if page.applet_id != applet.id || page.origin != applet.origin {
        return Err(Error::MismatchedApplet);
    }
    let (prime, account_id) = applet.encryption()?;
    let prime: Vec<u8> = serde_json::from_str(prime).map_err(|_| Error::InvalidModulus)?;
    if prime.iter().all(|byte| *byte == 0) {
        return Err(Error::InvalidModulus);
    }
    Ok(EncryptedData {
        page,
        prime,
        account_id: account_id.to_owned(),
    })
}

/// Decrypt a complete page without mutating the input or performing I/O.
pub fn decrypt(password: &str, data: &EncryptedData) -> DecryptedData {
    let mut page = data.page.clone();
    let modulus = BigUint::from_bytes_be(&data.prime);
    let private = [password, &data.account_id]
        .into_iter()
        .flat_map(|part| web_buffer_utf8(&Sha512::digest(part.as_bytes())).into_bytes())
        .collect::<Vec<_>>();
    let private = BigUint::from_bytes_be(&private);

    let outcomes = page
        .answers
        .iter_mut()
        .map(|record| {
            let has_ciphertext = ["answer", "events"]
                .into_iter()
                .any(|field| record.get(field).is_some_and(Value::is_string));
            let key = record
                .get("userPublicKey")
                .and_then(Value::as_str)
                .filter(|_| has_ciphertext)
                .and_then(|value| serde_json::from_str::<Vec<u8>>(value).ok())
                .map(|key| {
                    let secret = BigUint::from_bytes_be(&key)
                        .modpow(&private, &modulus)
                        .to_bytes_be();
                    let mut padded = vec![0; data.prime.len()];
                    let start = padded.len() - secret.len();
                    padded[start..].copy_from_slice(&secret);
                    <[u8; 32]>::from(Sha256::digest(padded))
                });
            AnswerOutcome {
                answer: decrypt_field(record, "answer", key.as_ref()),
                events: decrypt_field(record, "events", key.as_ref()),
            }
        })
        .collect();
    DecryptedData { page, outcomes }
}

// Match WebBuffer's width and code-point checks, including its treatment of
// nonstandard lead bytes and advancing one byte on an invalid sequence.
fn web_buffer_utf8(mut bytes: &[u8]) -> String {
    let mut result = String::new();
    while let Some(&lead) = bytes.first() {
        let (width, mask, min) = match lead {
            0x00..=0x7f => (1, 0x7f, 0),
            0xc0..=0xdf => (2, 0x1f, 0x80),
            0xe0..=0xef => (3, 0x0f, 0x800),
            0xf0..=0xff => (4, 0x07, 0x10000),
            _ => (1, 0, 1),
        };
        let point = bytes.get(1..width).and_then(|tail| {
            let value = tail
                .iter()
                .try_fold(u32::from(lead & mask), |value, byte| {
                    (byte & 0xc0 == 0x80).then_some((value << 6) | u32::from(byte & 0x3f))
                })?;
            (value >= min).then(|| char::from_u32(value)).flatten()
        });
        if let Some(point) = point {
            result.push(point);
            bytes = &bytes[width..];
        } else {
            result.push('\u{fffd}');
            bytes = &bytes[1..];
        }
    }
    result
}

fn decrypt_field(record: &mut Value, field: &str, key: Option<&[u8; 32]>) -> FieldOutcome {
    let Some(value) = record.get_mut(field) else {
        return FieldOutcome::Unchanged;
    };
    let Some(ciphertext) = value.as_str() else {
        return FieldOutcome::Unchanged;
    };
    let Some(key) = key else {
        return FieldOutcome::MissingOrInvalidKey;
    };
    let Some(plaintext) = decrypt_string(ciphertext, key) else {
        return FieldOutcome::Failed;
    };
    *value = serde_json::from_str(&plaintext).unwrap_or(Value::String(plaintext));
    FieldOutcome::Decrypted
}

fn decrypt_string(ciphertext: &str, key: &[u8; 32]) -> Option<String> {
    let (iv, ciphertext) = ciphertext.split_once(':')?;
    let iv = hex::decode(iv).ok()?;
    let ciphertext = hex::decode(ciphertext).ok()?;
    let plaintext = cbc::Decryptor::<aes::Aes256>::new_from_slices(key, &iv)
        .ok()?
        .decrypt_padded_vec::<Pkcs7>(&ciphertext)
        .ok()?;
    String::from_utf8(plaintext).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn legacy_utf8_tolerates_nonstandard_four_byte_lead_and_replaces_one_byte_at_a_time() {
        assert_eq!(
            web_buffer_utf8(&[0xfc, 0x80, 0x80, 0x80, 0xe2, 0x82]),
            "\u{100000}\u{fffd}\u{fffd}"
        );
    }

    #[test]
    fn decrypts_fixed_legacy_vector_without_mutating_the_input() {
        // Independently computed with the CTK derivation and OpenSSL AES-CBC;
        // this vector requires the one-byte UTF-8 replacement and left padding.
        let ciphertext = "00000000000000000000000000000000:614c0af56f96d3a6ec6162818b890cef";
        let input = EncryptedData {
            page: ExportPage {
                origin: "https://example.invalid".into(),
                applet_id: Uuid::nil(),
                answers: vec![
                    json!({"userPublicKey": "[5]", "answer": ciphertext, "events": "invalid", "newField": 9007199254740993_u64}),
                    json!({"userPublicKey": "not JSON", "answer": ciphertext, "events": null}),
                ],
                activities: vec![json!({"newField": [null, 7]})],
            },
            prime: vec![1, 1],
            account_id: "fixture-account".into(),
        };
        let output = decrypt("fixture-password", &input);
        assert_eq!(output.answers()[0]["answer"], json!([null, 7]));
        assert_eq!(output.answers()[0]["events"], "invalid");
        assert_eq!(
            output.answers()[0]["newField"],
            input.page.answers()[0]["newField"]
        );
        assert_eq!(output.answers()[1], input.page.answers()[1]);
        assert_eq!(output.activities(), input.page.activities());
        assert_eq!(input.page.answers()[0]["answer"], ciphertext);
        assert_eq!(
            output.outcomes(),
            [
                AnswerOutcome {
                    answer: FieldOutcome::Decrypted,
                    events: FieldOutcome::Failed
                },
                AnswerOutcome {
                    answer: FieldOutcome::MissingOrInvalidKey,
                    events: FieldOutcome::Unchanged
                },
            ]
        );
        assert!(!format!("{output:?}").contains("newField"));
    }
}
