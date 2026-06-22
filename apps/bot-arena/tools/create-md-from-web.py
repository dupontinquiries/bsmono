#!/usr/bin/env python3

import argparse
import concurrent.futures
import os
import re
import subprocess
from pathlib import Path

# Remove markdown images:
# ![alt](url)
IMAGE_RE = re.compile(r'!\[[^\]]*\]\([^)]*\)', re.S)

# Replace markdown links:
# [text](url) -> text
LINK_RE = re.compile(r'\[([^\]]+)\]\([^)]*\)', re.S)

# Collapse excessive blank lines
BLANK_RE = re.compile(r'\n{3,}')


def clean_markdown(md: str) -> str:
    md = IMAGE_RE.sub('', md)
    md = LINK_RE.sub(r'\1', md)
    md = BLANK_RE.sub('\n\n', md)
    return md.strip()


def convert_file(html_path: Path) -> str:
    try:
        proc = subprocess.run(
            ["html2text", str(html_path)],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            check=True,
        )

        cleaned = clean_markdown(proc.stdout)

        return f"# FILE: {html_path}\n\n{cleaned}\n\n"

    except Exception as e:
        return f"# FILE: {html_path}\n\nERROR: {e}\n\n"


def find_html_files(root: Path):
    for path in root.rglob("*"):
        if path.is_file() and path.suffix.lower() in [".html", ".htm"]:
            yield path


def main():
    parser = argparse.ArgumentParser(
        description="Convert HTML corpus to cleaned Markdown"
    )

    parser.add_argument(
        "input_dir",
        help="Top-level directory containing HTML files",
    )

    parser.add_argument(
        "output_file",
        help="Combined output markdown file",
    )

    parser.add_argument(
        "-j",
        "--jobs",
        type=int,
        default=4,
        help="Number of worker threads/processes (default: 4)",
    )

    args = parser.parse_args()

    root = Path(args.input_dir)
    output = Path(args.output_file)

    html_files = list(find_html_files(root))

    print(f"Found {len(html_files)} HTML files")
    print(f"Using {args.jobs} workers")

    with output.open("w", encoding="utf-8") as out_fp:
        with concurrent.futures.ThreadPoolExecutor(
            max_workers=args.jobs
        ) as executor:

            futures = [
                executor.submit(convert_file, path)
                for path in html_files
            ]

            for i, future in enumerate(
                concurrent.futures.as_completed(futures),
                start=1,
            ):
                result = future.result()
                out_fp.write(result)

                if i % 100 == 0:
                    print(f"Processed {i}/{len(html_files)}")

    print(f"Done: {output}")


if __name__ == "__main__":
    main()
