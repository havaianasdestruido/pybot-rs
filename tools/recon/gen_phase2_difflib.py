#!/usr/bin/env python3
"""Generate exhaustive short-string ratio vectors from CPython difflib.SequenceMatcher."""
from difflib import SequenceMatcher
from itertools import product
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tests" / "fixtures" / "phase2_difflib.tsv"


def words(max_length: int):
    yield ""
    for length in range(1, max_length + 1):
        yield from map("".join, product("ab", repeat=length))


def main() -> None:
    strings = list(words(5))
    rows = ["# CPython difflib.SequenceMatcher(None, a, b).ratio() exhaustive strings over {a,b}, length <= 5"]
    rows.extend(f"{a}\t{b}\t{SequenceMatcher(None, a, b).ratio()!r}" for a in strings for b in strings)
    content = "\n".join(rows) + "\n"
    if "--check" in sys.argv:
        if not OUT.exists() or OUT.read_text(encoding="ascii") != content:
            raise SystemExit(f"{OUT.relative_to(ROOT)} is stale; regenerate with {Path(__file__).relative_to(ROOT)}")
        print(f"verified {(len(strings))**2} difflib reference cases")
    else:
        OUT.write_text(content, encoding="ascii")
        print(f"wrote {(len(strings))**2} difflib reference cases")


if __name__ == "__main__":
    main()
