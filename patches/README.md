# Generated client patches

[`openapi-to-rust-strict-utf8.patch`](openapi-to-rust-strict-utf8.patch) makes the generated HTTP client validate UTF-8 before parsing successful JSON responses. Without it, `openapi-to-rust` v0.24.0 generates a client that can replace an invalid byte inside a JSON string with `�` and return the altered value as if the server sent it. For example, `activity ` followed by the invalid byte `0xFF` can come back as `activity �`. With the patch, the SDK returns a `ResponseShape` error. A genuine `�` encoded as valid UTF-8 is still accepted.

The fix is already present in `src/generated/client.rs`. Normal builds do not use this directory, but `just build-api` overwrites the generated client and then reapplies the patch. Deleting only the patch file would make that command fail after it regenerated the unpatched client.

Remove the patch and its `just build-api` step together once the upstream generator rejects malformed UTF-8, including in fields the typed decoder ignores. Verify with `cargo test --locked --test response_encoding`.
