"""Tests for the repository build script."""

from pathlib import Path
from unittest import TestCase

import build


class BuildScriptTests(TestCase):
    """Verify build.py constructs the intended command sequence."""

    def test_debug_build_commands_use_debug_wasm_output(self) -> None:
        """Debug builds should refresh assets before rebuilding localref."""
        root = Path("repo")

        commands = build.build_commands(root, release=False)

        self.assertEqual(commands[0], [build.npm_command(), "run", "build:css"])
        self.assertEqual(
            commands[1],
            [
                "cargo",
                "build",
                "-p",
                "ui-app",
                "--target",
                "wasm32-unknown-unknown",
                "--no-default-features",
                "--features",
                "hydrate",
            ],
        )
        self.assertEqual(
            commands[2][-1],
            str(
                root
                / "target"
                / "wasm32-unknown-unknown"
                / "debug"
                / "ui_app.wasm"
            ),
        )
        self.assertEqual(commands[3], ["cargo", "build", "-p", "localref"])

    def test_release_build_commands_use_release_profile(self) -> None:
        """Release builds should use release profile for both Rust builds."""
        root = Path("repo")

        commands = build.build_commands(root, release=True)

        self.assertIn("--release", commands[1])
        self.assertEqual(
            commands[2][-1],
            str(
                root
                / "target"
                / "wasm32-unknown-unknown"
                / "release"
                / "ui_app.wasm"
            ),
        )
        self.assertEqual(commands[3], ["cargo", "build", "-p", "localref", "--release"])


if __name__ == "__main__":
    import unittest

    unittest.main()
