#!/usr/bin/env python3
"""Build the Windows installer from a verified package directory.

Run after scripts/release/package.py on Windows with Inno Setup 6 installed:

    python scripts/release/build_installer.py --packages dist --output dist-installer

Writes GENORYN-Setup-<version>.exe and its .sha256 checksum. The installer contains
exactly the files of the verified package, including licenses and the manifest.
"""
import argparse
import hashlib
import os
import pathlib
import shutil
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[2]


def find_compiler():
    candidates = [
        os.environ.get("ISCC"),
        shutil.which("ISCC"),
        r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        r"C:\Program Files\Inno Setup 6\ISCC.exe",
        os.path.expandvars(r"%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"),
    ]
    for candidate in candidates:
        if candidate and pathlib.Path(candidate).is_file():
            return candidate
    raise SystemExit("Inno Setup 6 (ISCC.exe) was not found; install it or set ISCC")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--packages", default="dist")
    parser.add_argument("--output", default="dist-installer")
    parser.add_argument("--edition", choices=["full", "lite"], default="full")
    args = parser.parse_args()
    if os.name != "nt":
        raise SystemExit("The Windows installer is built on Windows")
    packages = pathlib.Path(args.packages).resolve()
    lite = args.edition == "lite"
    pattern = "genoryn-*-mainnet-lite-windows-*" if lite else "genoryn-*-mainnet-windows-*"
    stages = [p for p in packages.glob(pattern) if p.is_dir()]
    if len(stages) != 1:
        raise SystemExit(f"Expected exactly one {args.edition} Windows package directory in {packages}")
    stage = stages[0]
    if not (stage / "genoryn-wallet.exe").is_file() or not (stage / "release-manifest.json").is_file():
        raise SystemExit("The package directory is incomplete; run package.py first")
    version = stage.name.split("-")[1]
    output = pathlib.Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            find_compiler(),
            "/Qp",
            f"/DAppVersion={version}",
            f"/DSourceDir={stage}",
            f"/DRepoDir={ROOT}",
            f"/DOutputDir={output}",
            *(["/DLite=1"] if lite else []),
            str(ROOT / "packaging" / "windows" / "genoryn.iss"),
        ],
        check=True,
    )
    installer = output / (f"GENORYN-Lite-Setup-{version}.exe" if lite else f"GENORYN-Setup-{version}.exe")
    digest = hashlib.sha256(installer.read_bytes()).hexdigest()
    (output / f"{installer.name}.sha256").write_text(f"{digest}  {installer.name}\n")
    print(f'{{"installer": "{installer.as_posix()}", "sha256": "{digest}"}}')


if __name__ == "__main__":
    main()
