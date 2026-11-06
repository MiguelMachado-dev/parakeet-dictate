"""The Windows app's last Python version, which the Mac's reference tools still run. It is Rust from then on."""

import io
import subprocess
import tarfile
import tempfile
from pathlib import Path

COMMIT = "5d4e136"
ROOT = Path(__file__).resolve().parents[2]


def last_python_src() -> str:
    """Unpacks src/ from COMMIT into a temporary folder and returns its path."""
    archive = subprocess.run(["git", "-C", str(ROOT), "archive", COMMIT, "src/parakeet_dictate"], capture_output=True, check=True).stdout
    folder = tempfile.mkdtemp(prefix="parakeet-python-")
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(folder, filter="data")
    return str(Path(folder) / "src")
