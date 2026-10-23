#!/usr/bin/env python3
"""Create a local, versioned macOS preview bundle without replacing an old build."""
import argparse
import datetime
import pathlib
import plistlib
import shutil
import sys
import tempfile

try:
    import tomllib
except ModuleNotFoundError:
    raise SystemExit("Packaging requires Python 3.11 or newer; run this helper with a compatible python3.")


def occupied(path):
    try:
        path.lstat()
    except FileNotFoundError:
        return False
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=("debug", "release"), default="debug")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("This helper packages an already-built macOS binary on macOS.")
    root = pathlib.Path(__file__).resolve().parents[1]
    with (root / "packaging/macos/Info.plist").open("rb") as handle:
        metadata = plistlib.load(handle)
    version = metadata["CFBundleShortVersionString"]
    with (root / "Cargo.toml").open("rb") as handle:
        cargo_version = tomllib.load(handle)["workspace"]["package"]["version"]
    if version != cargo_version:
        parser.error("The bundle version must match the workspace release version.")
    executable_name = metadata["CFBundleExecutable"]
    display_name = metadata["CFBundleDisplayName"]
    binary = root / "target" / args.profile / executable_name
    if not binary.is_file():
        parser.error("Build devenv-studio for the selected profile before packaging.")
    output = root / "dist"
    output.mkdir(exist_ok=True)
    destination = output / (display_name + " " + version + ".app")
    if occupied(destination):
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
        destination = output / (display_name + " " + version + "-preview-" + stamp + ".app")
    staging = pathlib.Path(tempfile.mkdtemp(prefix=".devenv-bundle-", dir=output))
    try:
        contents = staging / "Contents"
        executable = contents / "MacOS" / executable_name
        executable.parent.mkdir(parents=True)
        resources = contents / "Resources"
        resources.mkdir()
        shutil.copy2(binary, executable)
        shutil.copy2(root / "packaging/macos/Info.plist", contents / "Info.plist")
        for name in ("LICENSE", "THIRD_PARTY_NOTICES.md", "README.md"):
            shutil.copy2(root / name, resources / name)
        for directory in ("third-party", "docs", "data", "examples"):
            shutil.copytree(root / directory, resources / directory)
        # link/rename replacement semantics vary; reject occupied paths immediately
        # before publishing this same-directory staging bundle.
        if occupied(destination):
            raise FileExistsError(str(destination))
        staging.rename(destination)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    print(destination)
    print("Local " + args.profile + " preview; signing and notarization were not performed.")


if __name__ == "__main__":
    main()
