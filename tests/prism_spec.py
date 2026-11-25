"""Add SDK smoke-test response examples to the complete Curious OpenAPI spec."""

import json
import pathlib
import sys
import urllib.error
import urllib.request
from typing import Final

_USER_AGENT: Final = (
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36"
)


def main() -> None:
    """Fetch the source schema and emit it with named examples for SDK tests."""
    fixtures = pathlib.Path(__file__).parent / "fixtures" / "prism-responses.json"
    # Curious rejects urllib's default request; use the SDK's User-Agent.
    request = urllib.request.Request(
        sys.argv[1], headers={"Accept": "application/json", "User-Agent": _USER_AGENT}
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            spec = json.load(response)
    except urllib.error.URLError as error:
        raise SystemExit(f"Could not fetch the Curious OpenAPI spec: {error}") from None
    examples = json.loads(fixtures.read_text(encoding="utf-8"))
    examples["/applets"] = {"result": [examples["/applets/{applet_id}"]["result"]], "count": 1}
    for path, example in examples.items():
        method = "post" if path == "/auth/login" else "get"
        operation = spec["paths"][path][method]
        body = operation["responses"]["200"]["content"]["application/json"]
        body.pop("example", None)
        body["examples"] = {"sdk_fixture": {"value": example}}
    json.dump(spec, sys.stdout)


if __name__ == "__main__":
    main()
