"""Offline smoke tests for the installed Python extension."""

import unittest

from curious_sdk import Client, CuriousError


class ClientTest(unittest.TestCase):
    """Exercise public Python validation and diagnostics without HTTP calls."""

    def test_rejects_non_loopback_mock_origins(self) -> None:
        """Do not send mock credentials outside loopback."""
        with self.assertRaisesRegex(CuriousError, "loopback"):
            Client.for_mock("http://192.0.2.1:4010", "synthetic-token")

    def test_client_repr_does_not_show_credentials(self) -> None:
        """Keep token contents out of incidental diagnostics."""
        client = Client("https://example.invalid", "synthetic-token")
        self.assertNotIn("synthetic-token", repr(client))


class AsyncClientTest(unittest.IsolatedAsyncioTestCase):
    """Exercise validation while an asyncio event loop is running."""

    async def test_invalid_export_date_does_not_schedule_a_request(self) -> None:
        """Reject a malformed date before creating the asyncio future."""
        client = Client("https://example.invalid")
        with self.assertRaisesRegex(CuriousError, "YYYY-MM-DD"):
            client.export_applet_data("00000000-0000-0000-0000-000000000000", from_date="2026-1-02")
