from __future__ import annotations

import os
import subprocess
import unittest
from pathlib import Path

from scripts import fork_release

DISCORD_NOTIFY = Path(__file__).resolve().parent / "discord_release_notify.sh"


def _assets(tag: str) -> tuple[dict[str, str], dict[str, str]]:
    assets = {
        target: f"https://github.com/shuv1337/herdr/releases/download/{tag}/{name}"
        for target, name in fork_release.ASSET_NAMES.items()
    }
    checksums = {target: "a" * 64 for target in fork_release.ASSET_NAMES}
    return assets, checksums


class ForkTagTests(unittest.TestCase):
    def test_parse_tag_accepts_fork_tags(self) -> None:
        self.assertEqual(fork_release.parse_tag("v0.9.1-shuv.3"), ("0.9.1", 3))

    def test_parse_tag_rejects_other_tags(self) -> None:
        for tag in ("v0.9.1", "v0.9.1-shuv.0", "0.9.1-shuv.1", "v0.9.1-preview.1", "v0.9.1-shuv.01"):
            with self.subTest(tag=tag), self.assertRaises(fork_release.ForkReleaseError):
                fork_release.parse_tag(tag)

    def test_versions_order_by_fork_revision(self) -> None:
        versions = ["0.9.1-shuv.10", "0.9.1", "0.9.2-shuv.1", "0.9.1-shuv.2"]
        self.assertEqual(
            sorted(versions, key=fork_release.version_key),
            ["0.9.1", "0.9.1-shuv.2", "0.9.1-shuv.10", "0.9.2-shuv.1"],
        )

    def test_next_and_previous_tags(self) -> None:
        tags = ["v0.9.0-shuv.4", "v0.9.1-shuv.1", "v0.9.1-shuv.2"]
        self.assertEqual(fork_release.next_fork_tag("0.9.1", tags), "v0.9.1-shuv.3")
        self.assertEqual(fork_release.next_fork_tag("0.9.2", tags), "v0.9.2-shuv.1")
        self.assertEqual(fork_release.previous_fork_tag("v0.9.1-shuv.1", tags), "v0.9.0-shuv.4")
        self.assertIsNone(fork_release.previous_fork_tag("v0.9.0-shuv.4", tags))


class ForkNotesTests(unittest.TestCase):
    def test_notes_list_fork_commits_and_upstream_base(self) -> None:
        notes = fork_release.build_notes(
            "v0.9.1-shuv.2",
            "v0.9.1-shuv.1",
            ["feat: thing (abc1234)"],
            "0123456789abcdef",
            7,
        )
        self.assertIn("### Fork changes\n- feat: thing (abc1234)", notes)
        self.assertIn("herdrdev/herdr@0123456789ab (7 new upstream commits since v0.9.1-shuv.1)", notes)
        self.assertIn("releases/latest/download/install.sh", notes)

    def test_notes_are_never_empty(self) -> None:
        notes = fork_release.build_notes("v0.9.1-shuv.1", None, [], None, None)
        self.assertIn("- No fork-only changes.", notes)


class ForkManifestTests(unittest.TestCase):
    def test_manifest_matches_update_manifest_shape(self) -> None:
        assets, checksums = _assets("v0.9.1-shuv.1")
        manifest = fork_release.build_manifest("0.9.1-shuv.1", "notes\n", assets, checksums, 22, 1)

        self.assertEqual(manifest["version"], "0.9.1-shuv.1")
        self.assertEqual(manifest["notes"], "notes")
        self.assertEqual(manifest["protocol"], 22)
        self.assertEqual(manifest["endpoint_generation"], 1)
        self.assertEqual(manifest["assets"], assets)
        self.assertEqual(manifest["sha256"], checksums)
        self.assertEqual(list(manifest["releases"]), ["0.9.1-shuv.1"])

    def test_manifest_keeps_previous_releases_newest_first(self) -> None:
        old_assets, old_checksums = _assets("v0.9.1-shuv.1")
        previous = fork_release.build_manifest("0.9.1-shuv.1", "old", old_assets, old_checksums, 21, 1)
        previous["releases"]["not-a-version"] = {"notes": "junk"}
        assets, checksums = _assets("v0.9.1-shuv.2")

        manifest = fork_release.build_manifest(
            "v0.9.1-shuv.2", "new", assets, checksums, 22, 1, previous
        )

        self.assertEqual(list(manifest["releases"]), ["0.9.1-shuv.2", "0.9.1-shuv.1"])
        self.assertEqual(manifest["releases"]["0.9.1-shuv.1"]["protocol"], 21)
        self.assertEqual(manifest["releases"]["0.9.1-shuv.2"]["assets"], assets)

    def test_manifest_requires_every_asset(self) -> None:
        assets, checksums = _assets("v0.9.1-shuv.1")
        del assets["windows-x86_64"]
        with self.assertRaises(fork_release.ForkReleaseError):
            fork_release.build_manifest("0.9.1-shuv.1", "notes", assets, checksums, 22, 1)


@unittest.skipUnless(os.name == "posix", "Discord release notifier requires a POSIX bash")
class DiscordNotifyTests(unittest.TestCase):
    def _run(self, *args: str) -> subprocess.CompletedProcess[str]:
        env = {key: value for key, value in os.environ.items() if key != "DISCORD_RELEASE_WEBHOOK_URL"}
        return subprocess.run(
            ["bash", str(DISCORD_NOTIFY), *args],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_dry_run_accepts_fork_prerelease_tags(self) -> None:
        result = self._run(
            "--project", "herdr",
            "--version", "v0.9.1-shuv.3",
            "--repo", "shuv1337/herdr",
            "--link", "release: https://github.com/shuv1337/herdr/releases/tag/v0.9.1-shuv.3",
            "--dry-run",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout.splitlines(),
            [
                "**herdr v0.9.1-shuv.3** is out",
                "tag: https://github.com/shuv1337/herdr/tree/v0.9.1-shuv.3",
                "release: https://github.com/shuv1337/herdr/releases/tag/v0.9.1-shuv.3",
            ],
        )

    def test_missing_webhook_is_a_skip_not_an_error(self) -> None:
        result = self._run("--project", "herdr", "--version", "v0.9.1-shuv.3")
        self.assertEqual(result.returncode, 2)

    def test_rejects_malformed_versions(self) -> None:
        for version in ("v0.9.1;id", "0.9", "v0.9.1-"):
            with self.subTest(version=version):
                result = self._run("--project", "herdr", "--version", version, "--dry-run")
                self.assertEqual(result.returncode, 1)


if __name__ == "__main__":
    unittest.main()
