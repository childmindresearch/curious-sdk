# Curious SDK
Reinder: WIP Slop factory. Stay tuned.

Read Curious (MindLogger) applet metadata and export answer data in Rust or `asyncio` Python. Decryption is local and returns a new value; it does not modify its input or make HTTP requests.

## Rust

```rust,no_run
use curious_sdk::{Client, NaiveDate, Paging, Uuid, decrypt, encrypted_data};

async fn read_page() -> Result<(), Box<dyn std::error::Error>> {
    let token = std::env::var("CURIOUS_TOKEN")?;
    let id = Uuid::parse_str(&std::env::var("CURIOUS_APPLET_ID")?)?;
    let client = Client::new("https://api-v2.gettingcurious.com", Some(&token))?;
    let applet = client.get_applet(id).await?;
    let from = NaiveDate::parse_from_str("2026-11-01", "%Y-%m-%d")?;
    let page = client.export_applet_data(id, Some(from), Paging::default()).await?;
    let input = encrypted_data(page, &applet)?;
    let result = decrypt(&std::env::var("CURIOUS_APPLET_PASSWORD")?, &input);
    assert_eq!(result.answers().len(), result.outcomes().len());
    Ok(())
}
```

## Python

Build a wheel with `uv build --wheel` from the repo root (or `maturin build --locked`) and install it in a Python 3.11+ environment.

```python
import asyncio
import os

from curious_sdk import Client, decrypt, encrypted_data


async def main() -> None:
    client = Client("https://api-v2.gettingcurious.com", os.environ["CURIOUS_TOKEN"])
    applet_id = os.environ["CURIOUS_APPLET_ID"]
    applet = await client.get_applet(applet_id)
    page = await client.export_applet_data(applet_id, from_date="2026-11-01", page_size=51)
    result = await decrypt(os.environ["CURIOUS_APPLET_PASSWORD"], encrypted_data(page, applet))
    assert len(result.answers) == len(result.outcomes)


asyncio.run(main())
```

Both bindings also offer JSON login. It returns a new authenticated client or an MFA challenge, and never replaces the original session. The SDK does not complete MFA or refresh tokens.

## Paging, retries, and decryption

- The default page size is 50. You can request more, but larger pages are more likely to hit Curious gateway timeouts. Try smaller pages if timeouts persist. The SDK reads one page at a time; it does not infer an end-of-export rule.
- An export retries reported non-200 statuses with jittered backoff and up to three total attempts. The same page and filters are sent each time. Transport and decoded-shape errors are not retried. Redirects are not followed. With the current generated client, an unreadable or oversized response body can mask its status; such a response is not retried. The generated client has an 8 MiB buffered-body limit.
- Returned pages contain decoded data, not HTTP headers or original body bytes. When the generated typed decoder succeeds it can discard unknown fields and normalize some values, such as timestamps. This does not provide lossless CTK handoff. Python accessors return independent decoded copies; non-integer JSON numbers are exposed as `decimal.Decimal`.
- A field that fails decryption retains its original value. Consult per-field outcomes rather than treating every returned string as plaintext. Successful legacy AES-CBC decryption does not authenticate the plaintext. Only compose metadata and exports obtained from the same trusted Curious origin and applet. Use trusted data: Python bounds active jobs, not the CPU cost of an individual job.

## Development

The private client under `src/generated/` is based on output from `openapi-to-rust` v0.24.0. Ordinary builds do not need the generator or OpenAPI spec. To regenerate, run `just build-api`. It uses `openapi-to-rust.toml` to fetch the published spec and select only the four operations the SDK calls and their reachable models, then applies a local patch so successful responses are decoded from the original bytes and invalid UTF-8 is rejected. This is not an offline, pinned-spec drift check.

Run the Rust unit and local HTTP regression tests with `cargo test --locked`. Prism tests are opt-in, and live checks require separately provisioned credentials.

### Releases

GitHub Actions runs the Rust tests and the installed-wheel Python tests on pushes and pull requests. To release, match the version in `Cargo.toml`, `bindings/python/Cargo.toml`, and `pyproject.toml`, update both Cargo lockfiles, and push the corresponding `vMAJOR.MINOR.PATCH` tag. After the tests pass, the workflow attaches the Rust `.crate` and a manylinux x86-64 wheel for CPython 3.11+ to a GitHub Release. It does not publish to crates.io or PyPI. The regular CI run does not contact Curious or start Prism.

### Prism integration tests

Start `just mock-sdk` in one terminal. It fetches the full OpenAPI spec from the same URL as `just build-api` and serves it through Prism with fixed synthetic response examples from `tests/fixtures/` added to four operations. The success tests explicitly request the named `sdk_fixture` example. In a second terminal, run the Rust integration tests:

```sh
just test-mock
```

After rebuilding and installing the Python wheel in your test environment, run:

```sh
CURIOUS_MOCK_URL=http://127.0.0.1:4010 python -m unittest discover -s bindings/python/tests
```

Use `CURIOUS_MOCK_URL` to override the Rust tests' default of `http://127.0.0.1:4010` as well. The SDK's explicit mock constructor only permits loopback HTTP and bypasses proxies; use synthetic credentials only. These tests check SDK behavior, not generated code. They omit `fromDate` because Prism validates against the spec's date-time type, while the SDK sends CTK's date-only value. They do not establish live API compatibility.
