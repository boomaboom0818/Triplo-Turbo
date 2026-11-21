"""Create a local, branded build view of the pinned private engine."""

import hashlib
import io
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib


def prepare(root):
    engine = root / "engine"
    if not (engine / "Cargo.toml").is_file():
        sys.exit("Private engine not initialized. Run ./triplo-turbo setup.")

    entry = subprocess.check_output(["git", "-C", root, "ls-files", "--stage", "--", "engine"], text=True)
    mode, revision, stage, path = entry.split()
    if (mode, stage, path) != ("160000", "0", "engine"):
        sys.exit("Expected a pinned engine submodule.")

    script_id = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()[:16]
    cache = root / "target" / "engine-source"
    cache.mkdir(parents=True, exist_ok=True)
    dest = cache / f"{revision}-{script_id}"
    if not (dest / "Cargo.toml").is_file():
        # Read committed files only; never copy credentials, local edits, or hand history.
        snapshot = subprocess.check_output([
            "git", "-C", engine, "archive", revision, "Cargo.toml", "Cargo.lock", "src", "models",
        ])
        with tempfile.TemporaryDirectory(dir=cache) as temp:
            source = Path(temp) / "source"
            with tarfile.open(fileobj=io.BytesIO(snapshot)) as archive:
                archive.extractall(source, filter="data")
            rename(source)
            try:
                source.rename(dest)
            except OSError:
                if not (dest / "Cargo.toml").is_file():
                    raise
    print(dest / "Cargo.toml")


def rename(source):
    manifest = source / "Cargo.toml"
    original = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]["name"]
    ident = original.replace("-", "_")
    title = original.title()
    for path in [manifest, source / "Cargo.lock", *sorted((source / "src").rglob("*.rs"))]:
        text = path.read_text(encoding="utf-8")
        # Rust identifiers and environment names need underscores; CLI, files and
        # Cargo package names use a hyphen. Human-readable labels use a space.
        text = text.replace(f"{ident}::", "triplo_turbo::")
        text = re.sub(re.escape(title) + r"(?=[A-Z0-9])", "TriploTurbo", text)
        text = text.replace(ident.upper() + "_", "TRIPLO_TURBO_")
        text = text.replace(ident.upper(), "TRIPLO TURBO")
        text = text.replace(title, "Triplo Turbo")
        text = text.replace(original, "triplo-turbo")
        path.write_text(text, encoding="utf-8")


if __name__ == "__main__":
    prepare(Path(sys.argv[1]))
