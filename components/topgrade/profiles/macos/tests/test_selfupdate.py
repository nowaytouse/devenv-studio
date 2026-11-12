#!/usr/bin/env python3.15
"""Verify the retired helper cannot replace the Studio launcher."""

from pathlib import Path
import subprocess
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[1] / "bin/topgrade-rustup-headsync"


class RetiredHelper(unittest.TestCase):
    def test_old_modes_leave_launcher_and_home_untouched(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            launcher = home / "topgrade"
            target = home / "project-engine"
            launcher.symlink_to(target)
            for arguments in ([], ["update"], ["rollback"], ["--dry-run"]):
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
                    self.assertIn("devenv-studio topgrade-engine", result.stderr)
                    self.assertEqual(launcher.readlink(), target)
                    self.assertEqual(list(home.iterdir()), [launcher])


if __name__ == "__main__":
    unittest.main()
