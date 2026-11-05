"""Download Parakeet Ultra (ONNX, fp32 and int8) into models/. Safe to rerun: files
already present and unchanged are skipped."""

import argparse
import os
import sys
from pathlib import Path

from huggingface_hub import HfApi, hf_hub_download

REPO = "Olicorne/parakeet-tdt-0.6b-v3-ultra-onnx"
REVISION = "b27f93c2238aa35e138996c005a62f1e8430f801"
FILES = (
    "config.json",
    "vocab.txt",
    "nemo128.onnx",
    "fp32/decoder_joint-model.onnx",
    "fp32/decoder_joint-model.onnx.data",
    "fp32/encoder-model.onnx",
    "fp32/encoder-model.onnx.data.000",
    "fp32/encoder-model.onnx.data.001",
    "int8/decoder_joint-model.int8.onnx",
    "int8/encoder-model.int8.onnx",
)
SUBFOLDERS = ("fp32", "int8")
DEST = Path(__file__).resolve().parent.parent / "models" / "parakeet-ultra"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dest", type=Path, default=DEST)
    parser.add_argument("--dry-run", action="store_true", help="list files and whether each would be downloaded")
    args = parser.parse_args()

    sizes = {info.path: info.size for info in HfApi().get_paths_info(REPO, list(FILES), revision=REVISION)}
    for path in FILES:
        # onnx-asr reads one flat folder; the repo keeps each precision in its own.
        target = args.dest / Path(path).name
        present = target.is_file() and target.stat().st_size == sizes[path]
        if args.dry_run:
            print(f"{'present ' if present else 'download'} {sizes[path] / 1e6:8.1f} MB  {path}")
        elif not present:
            os.replace(hf_hub_download(REPO, path, revision=REVISION, local_dir=args.dest), target)
    if args.dry_run:
        return 0

    for folder in SUBFOLDERS:
        if (args.dest / folder).is_dir():
            (args.dest / folder).rmdir()
    print(f"model ready in {args.dest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
