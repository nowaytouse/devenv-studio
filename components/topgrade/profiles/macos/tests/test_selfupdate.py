#!/usr/bin/env python3.15
"""Verify that the deployed helper delegates only rolling operations."""

from pathlib import Path
import subprocess
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[1] / "bin/topgrade-rustup-headsync"


class RollingHelper(unittest.TestCase):
    def test_rejects_old_modes_and_invalid_checkout_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            launcher = home / "topgrade"
            target = home / "project-engine"
            launcher.symlink_to(target)
            for arguments in ([], ["update"], ["rollback", directory], ["--dry-run"], ["update", "relative"]):
                with self.subTest(arguments=arguments):
                    result = subprocess.run(
                        ["/bin/sh", str(HELPER), *arguments],
                        env={"HOME": directory, "PATH": directory},
                        capture_output=True,
                        text=True,
                        timeout=5,
                        check=False,
                    )
                    self.assertEqual(result.returncode, 2)
                    self.assertEqual(launcher.readlink(), target)
                    self.assertEqual(list(home.iterdir()), [launcher])

    def test_dispatch_preserves_absolute_checkout_and_exit_status(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            updater = home / ".local/bin/devenv-studio"
            updater.parent.mkdir(parents=True)
            updater.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\nexit 7\n')
            updater.chmod(0o700)
            checkout = str(home / "Studio Checkout")
            for action in ("plan", "update"):
                with self.subTest(action=action):
                    result = subprocess.run(
                        ["/bin/sh", str(HELPER), action, checkout],
                        env={"HOME": directory, "PATH": directory},
                        capture_output=True, text=True, timeout=5, check=False,
                    )
                    self.assertEqual(result.returncode, 7)
                    self.assertEqual(result.stdout.splitlines(), ["topgrade-engine", action, checkout])


if __name__ == "__main__":
    unittest.main()
