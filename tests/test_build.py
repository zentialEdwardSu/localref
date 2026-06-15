"""Tests for the repository build script."""

import tempfile
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

    def test_example_plugins_are_built_after_native_binary(self) -> None:
        """Each example plugin manifest should get its own build command."""
        root = Path(build.__file__).resolve().parent

        commands = build.build_commands(root, release=False)

        plugin_manifests = sorted(root.glob("examples/plugins/*/Cargo.toml"))
        self.assertTrue(plugin_manifests, "expected example plugins to exist")
        expected = [
            ["cargo", "build", "--manifest-path", str(manifest)]
            for manifest in plugin_manifests
        ]
        self.assertEqual(commands[4:], expected)

    def test_example_plugins_use_release_profile(self) -> None:
        """Release builds should pass --release to each plugin build."""
        root = Path(build.__file__).resolve().parent

        commands = build.build_commands(root, release=True)

        plugin_commands = commands[4:]
        self.assertTrue(plugin_commands, "expected example plugin builds")
        for command in plugin_commands:
            self.assertEqual(command[:2], ["cargo", "build"])
            self.assertIn("--release", command)

    def test_stage_plugin_files_bundles_built_plugins(self) -> None:
        """Staging copies plugin.toml, ui.toml, and the built exe per plugin."""
        suffix = build.exe_suffix()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # Two example plugins; only one has a ui.toml.
            self._make_plugin(root, "bibtexer", ui=True)
            self._make_plugin(root, "hooklog", ui=False)
            # Pretend both executables were built into target/debug/.
            target = root / "target" / "debug"
            target.mkdir(parents=True)
            (target / f"bibtexer{suffix}").write_text("exe")
            (target / f"hooklog{suffix}").write_text("exe")

            pairs = build.stage_plugin_files(root, release=False)

            staging = target / "builtin-plugins"
            dests = {dest for _, dest in pairs}
            self.assertIn(staging / "bibtexer" / "plugin.toml", dests)
            self.assertIn(staging / "bibtexer" / "ui.toml", dests)
            self.assertIn(staging / "bibtexer" / f"bibtexer{suffix}", dests)
            self.assertIn(staging / "hooklog" / "plugin.toml", dests)
            self.assertIn(staging / "hooklog" / f"hooklog{suffix}", dests)
            # hooklog has no ui.toml, so none is staged for it.
            self.assertNotIn(staging / "hooklog" / "ui.toml", dests)

    def test_stage_plugin_files_skips_unbuilt_plugins(self) -> None:
        """A plugin with no built executable is skipped, not an error."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._make_plugin(root, "bibtexer", ui=True)
            (root / "target" / "release").mkdir(parents=True)

            pairs = build.stage_plugin_files(root, release=True)

            self.assertEqual(pairs, [])

    @staticmethod
    def _make_plugin(root: Path, name: str, ui: bool) -> None:
        """Create a minimal example plugin source layout under root."""
        plugin_dir = root / "examples" / "plugins" / name
        plugin_dir.mkdir(parents=True)
        (plugin_dir / "plugin.toml").write_text(f'name = "{name}"\n')
        if ui:
            (plugin_dir / "ui.toml").write_text("[[actions]]\n")


if __name__ == "__main__":
    import unittest

    unittest.main()
