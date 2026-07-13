"""Build the Localref Rust core (cdylib), C# bindings, and the Avalonia app.

Pipeline:
1. `cargo build -p localref-ffi` — the UniFFI cdylib (`localref_ffi.dll`).
2. `uniffi-bindgen-cs` — regenerate the C# bindings from the built library.
3. `cargo build -p localref` — the `localref-cli` binary (setup + headless).
4. Build the example plugins and stage their bundles under the target dir.
5. `dotnet publish` — the Avalonia desktop app (self-contained single-file exe,
   RID-parameterized).
6. Copy `localref_ffi.dll` + the tray/window icon beside the published exe.
7. Stage the built-in plugin bundles and `localref-cli` beside the published
   exe so the app installs the built-ins on first run (skip with
   `--no-stage-builtins`).

The old CSS/WASM/wasm-bindgen steps are gone with the Leptos UI.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

# Relative to the workspace root.
APP_PROJECT = Path("app") / "Localref.Desktop" / "Localref.Desktop.csproj"
GENERATED_DIR = Path("app") / "Localref.Desktop" / "Generated"
UNIFFI_CONFIG = Path("crates") / "ffi" / "uniffi.toml"
ICON_SOURCE = Path("assets") / "favicon.ico"
CDYLIB_STEM = "localref_ffi"
CLI_STEM = "localref-cli"


def main(argv: list[str] | None = None) -> int:
    """Parse command-line options and run the requested build."""
    args = parse_args(argv)
    root = Path(__file__).resolve().parent
    profile = "release" if args.release else "debug"

    build_cdylib(root, args.release)
    generate_bindings(root, profile)
    build_cli(root, args.release)
    build_plugins(root, args.release)
    stage_builtin_plugins(root, args.release)
    if not args.skip_app:
        publish_app(root, args.release, args.rid)
        stage_native_artifacts(root, args.release, args.rid)
        if not args.no_stage_builtins:
            stage_app_extras(root, args.release, args.rid)
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
    parser.add_argument(
        "--no-stage-builtins",
        action="store_true",
        help="do not copy built-in plugins + localref-cli beside the published app",
    )
    return parser.parse_args(argv)


def build_cli(root: Path, release: bool) -> None:
    """Build the `localref-cli` binary (first-run setup + headless server)."""
    command = ["cargo", "build", "-p", "localref"]
    if release:
        command.append("--release")
    run_checked(command, root)


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
    """Publish the Avalonia app as a single-file executable for the given RID.

    `PublishSingleFile` bundles the managed assemblies into one executable. The
    native `localref_ffi` cdylib is still copied beside the exe afterwards (see
    `stage_native_artifacts`) so `DllImport` resolves it from disk.
    """
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
            "-p:PublishSingleFile=true",
        ],
        root,
    )


def publish_dir(root: Path, release: bool, rid: str) -> Path:
    """Return the dotnet publish output directory for the given profile + RID."""
    config = "Release" if release else "Debug"
    return (
        root
        / "app"
        / "Localref.Desktop"
        / "bin"
        / config
        / "net10.0"
        / rid
        / "publish"
    )


def stage_native_artifacts(root: Path, release: bool, rid: str) -> None:
    """Copy the cdylib and icon next to the published executable.

    `DllImport("localref_ffi")` resolves the library from the exe directory, so
    it must sit beside `Localref.Desktop.exe`. The icon backs the tray + window.
    """
    profile = "release" if release else "debug"
    dest = publish_dir(root, release, rid)
    if not dest.is_dir():
        print(f"! publish dir not found: {dest}", flush=True)
        return
    library = root / "target" / profile / cdylib_filename()
    copy_file(library, dest / cdylib_filename())
    if ICON_SOURCE.is_file() or (root / ICON_SOURCE).is_file():
        copy_file(root / ICON_SOURCE, dest / "localref.ico")


def stage_app_extras(root: Path, release: bool, rid: str) -> None:
    """Copy the built-in plugin bundles and `localref-cli` beside the app.

    The app resolves built-ins from `<exe dir>/builtin-plugins` on first run
    (see `localref_host::init`), so the staged bundles must sit in the publish
    directory. The CLI is copied alongside for setup/headless use. Missing
    sources are skipped with a warning so a partial build does not fail here.
    """
    profile = "release" if release else "debug"
    dest = publish_dir(root, release, rid)
    if not dest.is_dir():
        print(f"! publish dir not found: {dest}", flush=True)
        return
    staging = root / "target" / profile / "builtin-plugins"
    if staging.is_dir():
        copy_tree(staging, dest / "builtin-plugins")
    else:
        print(f"! no staged built-in plugins at {staging}", flush=True)
    cli = root / "target" / profile / f"{CLI_STEM}{exe_suffix()}"
    if cli.is_file():
        copy_file(cli, dest / cli.name)
    else:
        print(f"! localref-cli not built at {cli}", flush=True)


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


def read_cargo_version(plugin_dir: Path) -> str | None:
    """Return the `[package] version` from a plugin's Cargo.toml, or None.

    The plugin's Cargo.toml is the single source of truth for its version.
    Returns None when the file is absent or unparseable so staging degrades to
    a version-less bundle rather than failing the build.
    """
    cargo = plugin_dir / "Cargo.toml"
    try:
        data = tomllib.loads(cargo.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return None
    version = data.get("package", {}).get("version")
    return version if isinstance(version, str) else None


def staged_plugin_toml(manifest_src: Path, version: str | None) -> str:
    """Return the plugin.toml text with a `version` line injected from Cargo.

    The version is prepended as its own line. It is skipped when the source
    manifest already declares a `version` (hand-authored wins) or when no
    version was resolved from Cargo.toml.
    """
    text = manifest_src.read_text(encoding="utf-8")
    if version is None:
        return text
    already_declared = any(
        line.lstrip().startswith("version") for line in text.splitlines()
    )
    if already_declared:
        return text
    return f'version = "{version}"\n{text}'


def stage_plugin_files(
    root: Path, release: bool
) -> tuple[list[tuple[Path, Path]], dict[Path, str | None]]:
    """Return copy pairs and per-manifest versions for built-in plugin bundles.

    Each example plugin is staged into
    `target/<profile>/builtin-plugins/<name>/` with its `plugin.toml`, optional
    `ui.toml`, and the freshly built executable. `localref-cli init` copies these
    bundles into the library's plugins directory. Plugins whose executable has
    not been built yet are skipped so staging never fails a partial build.

    Returns `(pairs, versions)` where `pairs` are `(source, dest)` copies and
    `versions` maps each staged `plugin.toml` dest to the version resolved from
    the plugin's Cargo.toml (None when unavailable). The manifest is included in
    `pairs` for path tracking; `stage_builtin_plugins` injects the version when
    writing it.
    """
    profile = "release" if release else "debug"
    target = root / "target" / profile
    staging_root = target / "builtin-plugins"
    suffix = exe_suffix()
    pairs: list[tuple[Path, Path]] = []
    versions: dict[Path, str | None] = {}
    for manifest in sorted(root.glob("examples/plugins/*/plugin.toml")):
        plugin_dir = manifest.parent
        name = plugin_dir.name
        executable = target / f"{name}{suffix}"
        if not executable.is_file():
            continue
        dest = staging_root / name
        manifest_dest = dest / "plugin.toml"
        pairs.append((manifest, manifest_dest))
        versions[manifest_dest] = read_cargo_version(plugin_dir)
        ui = plugin_dir / "ui.toml"
        if ui.is_file():
            pairs.append((ui, dest / "ui.toml"))
        pairs.append((executable, dest / f"{name}{suffix}"))
    return pairs, versions


def stage_builtin_plugins(root: Path, release: bool) -> None:
    """Copy built-in plugin bundles into the target staging directory.

    The bundle's `plugin.toml` is written with a `version` injected from the
    plugin's Cargo.toml; everything else is a verbatim copy.
    """
    pairs, versions = stage_plugin_files(root, release)
    for source, dest in pairs:
        dest.parent.mkdir(parents=True, exist_ok=True)
        if dest in versions:
            version = versions[dest]
            text = staged_plugin_toml(source, version)
            note = f"version {version}" if version else "no version"
            print(f"$ stage {source} -> {dest} ({note})", flush=True)
            dest.write_text(text, encoding="utf-8")
        else:
            copy_file(source, dest)


def copy_file(source: Path, dest: Path) -> None:
    """Copy one file, creating parent directories and logging the action."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    print(f"$ copy {source} -> {dest}", flush=True)
    shutil.copy2(source, dest)


def copy_tree(source: Path, dest: Path) -> None:
    """Recursively copy a directory tree, logging and overwriting existing files."""
    print(f"$ copy tree {source} -> {dest}", flush=True)
    shutil.copytree(source, dest, dirs_exist_ok=True)


def run_checked(command: list[str], root: Path) -> None:
    """Run one build command and fail loudly if it exits unsuccessfully."""
    print("$ " + " ".join(command), flush=True)
    subprocess.run(command, cwd=root, check=True)


if __name__ == "__main__":
    raise SystemExit(main())
