"""Build Localref frontend assets and the native binary."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path


def main(argv: list[str] | None = None) -> int:
    """Parse command-line options and run the requested build."""
    args = parse_args(argv)
    root = Path(__file__).resolve().parent
    for command in build_commands(root, args.release):
        run_checked(command, root)
    stage_builtin_plugins(root, args.release)
    return 0


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    """Parse build script arguments."""
    parser = argparse.ArgumentParser(
        description="Build CSS, hydrated WASM assets, then the localref binary."
    )
    parser.add_argument(
        "--release",
        action="store_true",
        help="build the WASM UI and localref binary with Cargo release profile",
    )
    return parser.parse_args(argv)


def build_commands(root: Path, release: bool) -> list[list[str]]:
    """Return the ordered commands that refresh assets and rebuild localref."""
    profile = "release" if release else "debug"
    wasm = root / "target" / "wasm32-unknown-unknown" / profile / "ui_app.wasm"
    wasm_build = [
        "cargo",
        "build",
        "-p",
        "ui-app",
        "--target",
        "wasm32-unknown-unknown",
        "--no-default-features",
        "--features",
        "hydrate",
    ]
    native_build = ["cargo", "build", "-p", "localref"]
    if release:
        wasm_build.append("--release")
        native_build.append("--release")
    return [
        [npm_command(), "run", "build:css"],
        wasm_build,
        [
            "wasm-bindgen",
            "--target",
            "web",
            "--out-dir",
            "assets",
            "--out-name",
            "localref-ui-bindgen",
            str(wasm),
        ],
        native_build,
        *plugin_commands(root, release),
    ]


def plugin_commands(root: Path, release: bool) -> list[list[str]]:
    """Return one build command per example plugin under examples/plugins.

    Each plugin is its own workspace member; discovering them by manifest keeps
    new example plugins building without editing this script.
    """
    commands = []
    for manifest in sorted(root.glob("examples/plugins/*/Cargo.toml")):
        build = ["cargo", "build", "--manifest-path", str(manifest)]
        if release:
            build.append("--release")
        commands.append(build)
    return commands


def exe_suffix() -> str:
    """Return the platform executable suffix Cargo emits."""
    return ".exe" if sys.platform == "win32" else ""


def stage_plugin_files(root: Path, release: bool) -> list[tuple[Path, Path]]:
    """Return (source, dest) copy pairs that assemble built-in plugin bundles.

    Each example plugin is staged into
    `target/<profile>/builtin-plugins/<name>/` with its `plugin.toml`, optional
    `ui.toml`, and the freshly built executable. `localref init` copies these
    bundles into the library's plugins directory. Plugins whose executable has
    not been built yet are skipped so staging never fails a partial build.
    """
    profile = "release" if release else "debug"
    target = root / "target" / profile
    staging_root = target / "builtin-plugins"
    suffix = exe_suffix()
    pairs: list[tuple[Path, Path]] = []
    for manifest in sorted(root.glob("examples/plugins/*/plugin.toml")):
        plugin_dir = manifest.parent
        name = plugin_dir.name
        executable = target / f"{name}{suffix}"
        if not executable.is_file():
            continue
        dest = staging_root / name
        pairs.append((manifest, dest / "plugin.toml"))
        ui = plugin_dir / "ui.toml"
        if ui.is_file():
            pairs.append((ui, dest / "ui.toml"))
        pairs.append((executable, dest / f"{name}{suffix}"))
    return pairs


def stage_builtin_plugins(root: Path, release: bool) -> None:
    """Copy built-in plugin bundles into the target staging directory."""
    for source, dest in stage_plugin_files(root, release):
        dest.parent.mkdir(parents=True, exist_ok=True)
        print(f"$ copy {source} -> {dest}", flush=True)
        shutil.copy2(source, dest)


def npm_command() -> str:
    """Return the platform-specific npm executable name."""
    return "npm.cmd" if sys.platform == "win32" else "npm"


def run_checked(command: list[str], root: Path) -> None:
    """Run one build command and fail loudly if it exits unsuccessfully."""
    print("$ " + " ".join(command), flush=True)
    subprocess.run(command, cwd=root, check=True)


if __name__ == "__main__":
    raise SystemExit(main())
