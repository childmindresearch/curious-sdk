"""Opt-in tests of the installed asyncio SDK against the Prism mock."""

import os
import unittest
from typing import Final

from curious_sdk import Client, HttpStatusError, decrypt, encrypted_data

_MOCK_URL: Final = os.environ.get("CURIOUS_MOCK_URL")
_APPLET_ID: Final = "00000000-0000-4000-8000-000000000001"


@unittest.skipUnless(_MOCK_URL, "set CURIOUS_MOCK_URL to opt in to the Prism tests")
class PrismTest(unittest.IsolatedAsyncioTestCase):
    """Exercise SDK workflows with synthetic data over real HTTP."""

    async def test_exports_and_decrypts_one_page(self) -> None:
        """The installed binding returns a new page with decoded answers."""
        assert _MOCK_URL is not None
        anonymous = Client.for_mock(_MOCK_URL, prefer="example=sdk_fixture")
        client = await anonymous.login("fixture@example.invalid", "fixture-login")
        assert isinstance(client, Client)
        applet = await client.get_applet(_APPLET_ID)
        self.assertEqual(applet.name, "Prism SDK fixture")
        page = await client.export_applet_data(_APPLET_ID, page_size=51)
        self.assertEqual(len(page.answers), 1)
        result = await decrypt("fixture-password", encrypted_data(page, applet))
        self.assertEqual(result.answers[0]["answer"], [None, 7])
        self.assertEqual(result.outcomes, [("decrypted", "unchanged")])
        self.assertIsInstance(page.answers[0]["answer"], str)

    async def test_exhausted_retries_preserve_status_and_attempts(self) -> None:
        """Prism's forced 401 is retried and reported by the binding."""
        assert _MOCK_URL is not None
        client = Client.for_mock(_MOCK_URL, "fixture-token", prefer="code=401")
        with self.assertRaises(HttpStatusError) as raised:
            await client.export_applet_data(_APPLET_ID)
        self.assertEqual(raised.exception.status, 401)
        self.assertEqual(raised.exception.attempts, 3)
