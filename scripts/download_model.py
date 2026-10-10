"""Download Parakeet TDT 0.6B v3 (ONNX, fp32 and int8) into models/. Safe to rerun: files
already present and unchanged are skipped."""

import argparse
import sys
from pathlib import Path

from huggingface_hub import snapshot_download

REPO = "istupakov/parakeet-tdt-0.6b-v3-onnx"
DEST = Path(__file__).resolve().parent.parent / "models" / "parakeet-tdt-0.6b-v3"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dest", type=Path, default=DEST)
    parser.add_argument("--dry-run", action="store_true", help="list files and whether each would be downloaded")
    args = parser.parse_args()

    if args.dry_run:
        for info in snapshot_download(REPO, local_dir=args.dest, dry_run=True):
            size = f"{info.file_size / 1e6:8.1f} MB" if info.file_size else "        ?"
            print(f"{'download' if info.will_download else 'present '} {size}  {info.filename}")
        return 0

    print(f"model ready in {snapshot_download(REPO, local_dir=args.dest)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
