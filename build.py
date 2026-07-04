"""Build the Localref Rust core (cdylib), C# bindings, and the Avalonia app.

Pipeline:
1. `cargo build -p localref-ffi` — the UniFFI cdylib (`localref_ffi.dll`).
2. `uniffi-bindgen-cs` — regenerate the C# bindings from the built library.
3. `dotnet publish` — the Avalonia desktop app (self-contained, RID-parameterized).
4. Copy `localref_ffi.dll` + the tray/window icon beside the published exe.
5. Stage the example plugins so `localref-cli init` can install them.

The old CSS/WASM/wasm-bindgen steps are gone with the Leptos UI.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

# Relative to the workspace root.
APP_PROJECT = Path("app") / "Localref.Desktop" / "Localref.Desktop.csproj"
GENERATED_DIR = Path("app") / "Localref.Desktop" / "Generated"
UNIFFI_CONFIG = Path("crates") / "ffi" / "uniffi.toml"
ICON_SOURCE = Path("assets") / "favicon.ico"
CDYLIB_STEM = "localref_ffi"


def main(argv: list[str] | None = None) -> int:
    """Parse command-line options and run the requested build."""
    args = parse_args(argv)
    root = Path(__file__).resolve().parent
    profile = "release" if args.release else "debug"

    build_cdylib(root, args.release)
    generate_bindings(root, profile)
    if not args.skip_app:
        publish_app(root, args.release, args.rid)
        stage_native_artifacts(root, args.release, args.rid)
    build_plugins(root, args.release)
    stage_builtin_plugins(root, args.release)
    return 0


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    """Parse build script arguments."""
    parser = argparse.ArgumentParser(
        description="Build the Rust cdylib, C# bindings, and Avalonia app."
    )
    parser.add_argument(
        "--release",
        action="store_true",
        help="build with the Cargo release profile and dotnet Release config",
    )
    parser.add_argument(
        "--rid",
        default="win-x64",
        help="dotnet runtime identifier (default win-x64; e.g. osx-arm64)",
    )
    parser.add_argument(
        "--skip-app",
        action="store_true",
        help="build only the Rust cdylib and regenerate bindings (skip dotnet)",
    )
    return parser.parse_args(argv)


def build_cdylib(root: Path, release: bool) -> None:
    """Build the UniFFI cdylib that the C# bindings load."""
    command = ["cargo", "build", "-p", "localref-ffi"]
    if release:
        command.append("--release")
    run_checked(command, root)


def generate_bindings(root: Path, profile: str) -> None:
    """Regenerate the C# bindings from the freshly built cdylib.

    Requires `uniffi-bindgen-cs` on PATH (install from NordSecurity's repo, tag
    matching the `uniffi` crate version). Skipped with a warning if absent so a
    Rust-only rebuild still succeeds.
    """
    library = root / "target" / profile / cdylib_filename()
    if shutil.which("uniffi-bindgen-cs") is None:
        print(
            "! uniffi-bindgen-cs not found on PATH; keeping existing bindings",
            flush=True,
        )
        return
    run_checked(
        [
            "uniffi-bindgen-cs",
            "--library",
            str(library),
            "--config",
            str(root / UNIFFI_CONFIG),
            "--out-dir",
            str(root / GENERATED_DIR),
        ],
        root,
    )


def publish_app(root: Path, release: bool, rid: str) -> None:
    """Publish the Avalonia app for the requested runtime identifier."""
    config = "Release" if release else "Debug"
    run_checked(
        [
            "dotnet",
            "publish",
            str(root / APP_PROJECT),
            "-c",
            config,
            "-r",
            rid,
            "--self-contained",
        ],
        root,
    )


def stage_native_artifacts(root: Path, release: bool, rid: str) -> None:
    """Copy the cdylib and icon next to the published executable.

    `DllImport("localref_ffi")` resolves the library from the exe directory, so
    it must sit beside `Localref.Desktop.exe`. The icon backs the tray + window.
    """
    profile = "release" if release else "debug"
    config = "Release" if release else "Debug"
    publish_dir = (
        root
        / "app"
        / "Localref.Desktop"
        / "bin"
        / config
        / "net10.0"
        / rid
        / "publish"
    )
    if not publish_dir.is_dir():
        print(f"! publish dir not found: {publish_dir}", flush=True)
        return
    library = root / "target" / profile / cdylib_filename()
    copy_file(library, publish_dir / cdylib_filename())
    if ICON_SOURCE.is_file() or (root / ICON_SOURCE).is_file():
        copy_file(root / ICON_SOURCE, publish_dir / "localref.ico")


def build_plugins(root: Path, release: bool) -> None:
    """Build every example plugin (each is its own workspace member)."""
    for manifest in sorted(root.glob("examples/plugins/*/Cargo.toml")):
        command = ["cargo", "build", "--manifest-path", str(manifest)]
        if release:
            command.append("--release")
        run_checked(command, root)


def cdylib_filename() -> str:
    """Return the platform filename Cargo emits for the cdylib."""
    if sys.platform == "win32":
        return f"{CDYLIB_STEM}.dll"
    if sys.platform == "darwin":
        return f"lib{CDYLIB_STEM}.dylib"
    return f"lib{CDYLIB_STEM}.so"


def exe_suffix() -> str:
    """Return the platform executable suffix Cargo emits."""
    return ".exe" if sys.platform == "win32" else ""


def stage_plugin_files(root: Path, release: bool) -> list[tuple[Path, Path]]:
    """Return (source, dest) copy pairs that assemble built-in plugin bundles.

    Each example plugin is staged into
    `target/<profile>/builtin-plugins/<name>/` with its `plugin.toml`, optional
    `ui.toml`, and the freshly built executable. `localref-cli init` copies these
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
        copy_file(source, dest)


def copy_file(source: Path, dest: Path) -> None:
    """Copy one file, creating parent directories and logging the action."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    print(f"$ copy {source} -> {dest}", flush=True)
    shutil.copy2(source, dest)


def run_checked(command: list[str], root: Path) -> None:
    """Run one build command and fail loudly if it exits unsuccessfully."""
    print("$ " + " ".join(command), flush=True)
    subprocess.run(command, cwd=root, check=True)


if __name__ == "__main__":
    raise SystemExit(main())
