"""Build Localref frontend assets and the native binary."""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path


def main(argv: list[str] | None = None) -> int:
    """Parse command-line options and run the requested build."""
    args = parse_args(argv)
    root = Path(__file__).resolve().parent
    for command in build_commands(root, args.release):
        run_checked(command, root)
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
    ]


def npm_command() -> str:
    """Return the platform-specific npm executable name."""
    return "npm.cmd" if sys.platform == "win32" else "npm"


def run_checked(command: list[str], root: Path) -> None:
    """Run one build command and fail loudly if it exits unsuccessfully."""
    print("$ " + " ".join(command), flush=True)
    subprocess.run(command, cwd=root, check=True)


if __name__ == "__main__":
    raise SystemExit(main())
