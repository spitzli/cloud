import copy
import unittest

from catalog_presentation import validate_presentation


class CatalogPresentation(unittest.TestCase):
    def setUp(self):
        self.ref = "a" * 40
        self.base = f"https://raw.githubusercontent.com/spitzli/cloud-plugin-neon/{self.ref}/"
        self.detail = {
            "id": "neon", "repoUrl": "https://github.com/spitzli/cloud-plugin-neon",
            "iconUrl": self.base + "assets/icon.svg", "readmeHtml": "<h1>Neon</h1><p>Database setup.</p>",
            "screenshots": [{"url": self.base + "screenshots/database.png", "alt": "Database details"}],
        }
        self.assets = {
            self.detail["iconUrl"]: (200, "image/svg+xml", b'<svg xmlns="http://www.w3.org/2000/svg"/>'),
            self.detail["screenshots"][0]["url"]: (200, "image/png", b"\x89PNG\r\n\x1a\nfixture"),
            self.base + "README.md": (200, "text/plain", b"# Neon\nDatabase setup instructions.\n"),
        }

    def check(self, detail=None):
        return validate_presentation(detail or self.detail, "neon", self.ref, self.assets.__getitem__)

    def test_public_presentation_is_required_not_just_a_manifest(self):
        self.assertEqual(self.check()["checkedAssets"], 3)
        for field in ["iconUrl", "readmeHtml", "screenshots"]:
            data = copy.deepcopy(self.detail)
            data[field] = None
            with self.assertRaises(ValueError):
                self.check(data)

    def test_wrong_registry_item_and_mutable_source_are_rejected(self):
        for field, value in [("id", "other"), ("repoUrl", "https://github.com/other/cloud-plugin-neon"),
                             ("iconUrl", self.base.replace(self.ref, "HEAD") + "assets/icon.svg")]:
            data = copy.deepcopy(self.detail)
            data[field] = value
            with self.assertRaises(ValueError):
                self.check(data)

    def test_inaccessible_assets_or_html_instead_of_an_image_fail(self):
        for response in [(404, "text/html", b"missing"), (200, "text/html", b"<html>error</html>"),
                         (200, "image/svg+xml", b"")]:
            self.assets[self.detail["iconUrl"]] = response
            with self.assertRaises(ValueError):
                self.check()

    def test_missing_screenshot_caption_is_not_a_valid_gallery(self):
        data = copy.deepcopy(self.detail)
        data["screenshots"][0]["alt"] = ""
        with self.assertRaises(ValueError):
            self.check(data)

    def test_asset_urls_cannot_escape_the_reviewed_repository(self):
        for path in ["https://127.0.0.1/secret.png", self.base + "../private.png",
                     self.base + "screenshots/database.png?token=private"]:
            data = copy.deepcopy(self.detail)
            data["screenshots"][0]["url"] = path
            with self.assertRaises(ValueError):
                self.check(data)
