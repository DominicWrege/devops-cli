"""Release packaging helpers. Python runs in CI, never on the user's machine."""

import argparse
import hashlib
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile


TARGETS = {
    "x86_64-unknown-linux-musl": "devops",
    "x86_64-pc-windows-msvc": "devops.exe",
}


def validate(tag, manifest=Path("Cargo.toml")):
    version = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]["version"]
    if not re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", tag):
        raise ValueError("Expected a vMAJOR.MINOR.PATCH tag, optionally with a prerelease suffix")
    if tag != f"v{version}":
        raise ValueError(f"Tag {tag!r} does not match Cargo.toml version {version!r}")
    return version


def verify_static(binary):
    headers = subprocess.run(
        ["readelf", "--program-headers", str(binary)],
        check=True, capture_output=True, text=True,
    ).stdout
    dynamic = subprocess.run(
        ["readelf", "--dynamic", str(binary)],
        check=True, capture_output=True, text=True,
    ).stdout
    if re.search(r"\bINTERP\b", headers) or "(NEEDED)" in dynamic:
        raise ValueError("Linux release must have no ELF interpreter or shared-library dependencies")


def package(version, target, binary, destination, root=Path(".")):
    executable = TARGETS[target]
    destination.mkdir(parents=True, exist_ok=True)
    name = f"devops-v{version}-{target}"
    extension = ".zip" if executable.endswith(".exe") else ".tar.gz"
    archive = destination / f"{name}{extension}"
    with tempfile.TemporaryDirectory() as temporary:
        stage = Path(temporary) / name
        stage.mkdir()
        shutil.copy2(binary, stage / executable)
        if not executable.endswith(".exe"):
            (stage / executable).chmod(0o755)
        shutil.copy2(root / "README.md", stage / "README.md")
        shutil.copytree(root / "LICENSES", stage / "LICENSES")
        if extension == ".zip":
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as output:
                for file in sorted(stage.rglob("*")):
                    if file.is_file():
                        output.write(file, file.relative_to(stage.parent))
        else:
            with tarfile.open(archive, "w:gz") as output:
                output.add(stage, arcname=name)
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(
        f"{digest}  {archive.name}\n", encoding="utf-8"
    )
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("validate")
    check.add_argument("tag")
    static = commands.add_parser("verify-static")
    static.add_argument("binary", type=Path)
    pack = commands.add_parser("package")
    pack.add_argument("version")
    pack.add_argument("target", choices=TARGETS)
    pack.add_argument("binary", type=Path)
    pack.add_argument("destination", type=Path)
    args = parser.parse_args()
    if args.command == "validate":
        print(validate(args.tag))
    elif args.command == "verify-static":
        verify_static(args.binary)
        print(f"Verified static ELF: {args.binary}")
    else:
        print(package(args.version, args.target, args.binary, args.destination))


if __name__ == "__main__":
    main()
