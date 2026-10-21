#!/usr/bin/env python3
"""Create a local, versioned macOS preview bundle without replacing an old build."""
import argparse
import pathlib
import plistlib
import shutil
import sys
import tempfile


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
    binary = root / "target" / args.profile / "ltsc-studio"
    if not binary.is_file():
        parser.error("Build ltsc-studio for the selected profile before packaging.")
    output = root / "dist"
    output.mkdir(exist_ok=True)
    destination = output / ("LTSC Studio " + version + ".app")
    if destination.exists():
        parser.error("Existing preview bundle is preserved: " + str(destination))
    staging = pathlib.Path(tempfile.mkdtemp(prefix=".ltsc-bundle-", dir=output))
    try:
        contents = staging / "Contents"
        executable = contents / "MacOS/ltsc-studio"
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
        if destination.exists():
            raise FileExistsError(str(destination))
        staging.rename(destination)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    print(destination)
    print("Local " + args.profile + " preview; signing and notarization were not performed.")


if __name__ == "__main__":
    main()
