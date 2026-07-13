"""Tests for the repository build script."""

import tempfile
from pathlib import Path
from unittest import TestCase

import build


class BuildScriptTests(TestCase):
    """Verify build.py constructs the intended command sequence."""

    def test_build_cli_uses_debug_profile_by_default(self) -> None:
        """The CLI build targets the `localref` package without --release."""
        captured: list[list[str]] = []
        original = build.run_checked
        build.run_checked = lambda cmd, root: captured.append(cmd)
        try:
            build.build_cli(Path("repo"), release=False)
        finally:
            build.run_checked = original

        self.assertEqual(captured, [["cargo", "build", "-p", "localref"]])

    def test_build_cli_uses_release_profile(self) -> None:
        """A release build passes --release to the CLI build."""
        captured: list[list[str]] = []
        original = build.run_checked
        build.run_checked = lambda cmd, root: captured.append(cmd)
        try:
            build.build_cli(Path("repo"), release=True)
        finally:
            build.run_checked = original

        self.assertEqual(
            captured, [["cargo", "build", "-p", "localref", "--release"]]
        )

    def test_stage_app_extras_copies_builtins_and_cli(self) -> None:
        """Staged bundles and the CLI land beside the published exe."""
        suffix = build.exe_suffix()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # A staged built-in bundle under target/debug/builtin-plugins.
            staging = root / "target" / "debug" / "builtin-plugins" / "bibtexer"
            staging.mkdir(parents=True)
            (staging / "plugin.toml").write_text('name = "bibtexer"\n')
            (staging / f"bibtexer{suffix}").write_text("exe")
            # The built CLI binary.
            cli = root / "target" / "debug" / f"{build.CLI_STEM}{suffix}"
            cli.write_text("cli")
            # An existing publish directory (dotnet publish output).
            dest = build.publish_dir(root, release=False, rid="win-x64")
            dest.mkdir(parents=True)

            build.stage_app_extras(root, release=False, rid="win-x64")

            self.assertTrue(
                (dest / "builtin-plugins" / "bibtexer" / "plugin.toml").is_file()
            )
            self.assertTrue(
                (dest / "builtin-plugins" / "bibtexer" / f"bibtexer{suffix}")
                .is_file()
            )
            self.assertTrue((dest / f"{build.CLI_STEM}{suffix}").is_file())

    def test_stage_app_extras_skips_missing_publish_dir(self) -> None:
        """No publish dir means nothing is copied and no error is raised."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # Build the staging inputs but omit the publish directory.
            staging = root / "target" / "debug" / "builtin-plugins" / "bibtexer"
            staging.mkdir(parents=True)
            (staging / "plugin.toml").write_text('name = "bibtexer"\n')

            # Must not raise even though the publish dir is absent.
            build.stage_app_extras(root, release=False, rid="win-x64")

            dest = build.publish_dir(root, release=False, rid="win-x64")
            self.assertFalse(dest.exists())

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

            pairs, _versions = build.stage_plugin_files(root, release=False)

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

            pairs, versions = build.stage_plugin_files(root, release=True)

            self.assertEqual(pairs, [])
            self.assertEqual(versions, {})

    def test_stage_injects_cargo_version_into_plugin_toml(self) -> None:
        """The staged plugin.toml gains a version line read from Cargo.toml."""
        suffix = build.exe_suffix()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._make_plugin(root, "s3sync", ui=False, version="1.2.3")
            target = root / "target" / "debug"
            target.mkdir(parents=True)
            (target / f"s3sync{suffix}").write_text("exe")

            build.stage_builtin_plugins(root, release=False)

            staged = (
                target / "builtin-plugins" / "s3sync" / "plugin.toml"
            ).read_text()
            self.assertIn('version = "1.2.3"', staged)
            # The original identity content is preserved.
            self.assertIn('name = "s3sync"', staged)

    def test_stage_without_cargo_version_omits_version_line(self) -> None:
        """A plugin whose Cargo.toml lacks a version stages no version line."""
        suffix = build.exe_suffix()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # No Cargo.toml at all -> read_cargo_version returns None.
            self._make_plugin(root, "hooklog", ui=False, version=None)
            target = root / "target" / "debug"
            target.mkdir(parents=True)
            (target / f"hooklog{suffix}").write_text("exe")

            build.stage_builtin_plugins(root, release=False)

            staged = (
                target / "builtin-plugins" / "hooklog" / "plugin.toml"
            ).read_text()
            self.assertNotIn("version", staged)

    @staticmethod
    def _make_plugin(
        root: Path, name: str, ui: bool, version: str | None = None
    ) -> None:
        """Create a minimal example plugin source layout under root.

        When `version` is given, a Cargo.toml carrying it is written alongside
        plugin.toml so version-injection can be exercised.
        """
        plugin_dir = root / "examples" / "plugins" / name
        plugin_dir.mkdir(parents=True)
        (plugin_dir / "plugin.toml").write_text(f'name = "{name}"\n')
        if version is not None:
            (plugin_dir / "Cargo.toml").write_text(
                f'[package]\nname = "{name}"\nversion = "{version}"\n'
            )
        if ui:
            (plugin_dir / "ui.toml").write_text("[[actions]]\n")


if __name__ == "__main__":
    import unittest

    unittest.main()
