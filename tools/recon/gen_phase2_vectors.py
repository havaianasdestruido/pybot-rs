#!/usr/bin/env python3
"""Export CPython integer-seeded random vectors for the Rust differential tests."""
from pathlib import Path
import random
import sys

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tests" / "fixtures" / "phase2_rng.tsv"
SEEDS = list(range(-128, 128)) + [2**64 + 17, -(2**96 + 3), 2**127 - 1, -(2**127)]


def row(seed: int) -> str:
    rng = random.Random(seed)
    floats = ",".join(repr(rng.random()) for _ in range(8))
    rng = random.Random(seed)
    bits = ",".join(str(rng.getrandbits(k)) for k in (1, 5, 32, 33, 64, 127))
    rng = random.Random(seed)
    choices = ",".join(str(rng.choice(range(101))) for _ in range(12))
    rng = random.Random(seed)
    values = list(range(24))
    rng.shuffle(values)
    rng = random.Random(seed)
    ranges = ",".join(str(rng.randrange(-50, 100, 7)) for _ in range(8))
    rng = random.Random(seed)
    pool_sample = ",".join(map(str, rng.sample(range(20), 5)))
    rng = random.Random(seed)
    small_sample = ",".join(map(str, rng.sample(range(30), 5)))
    rng = random.Random(seed)
    large_sample = ",".join(map(str, rng.sample(range(1000), 7)))
    return f"{seed}\t{floats}\t{bits}\t{choices}\t{','.join(map(str, values))}\t{ranges}\t{pool_sample}\t{small_sample}\t{large_sample}"


def main() -> None:
    OUT.parent.mkdir(parents=True, exist_ok=True)
    content = "# CPython random.Random vectors; seed | random | bits | choice | shuffle | randrange(-50,100,7) | sample(range(20),5) | sample(range(30),5) | sample(range(1000),7)\n"
    content += "\n".join(row(seed) for seed in SEEDS) + "\n"
    if "--check" in sys.argv:
        if not OUT.exists() or OUT.read_text(encoding="ascii") != content:
            raise SystemExit(f"{OUT.relative_to(ROOT)} is stale; regenerate with {Path(__file__).relative_to(ROOT)}")
        print(f"verified {len(SEEDS)} reference seeds in {OUT.relative_to(ROOT)}")
    else:
        OUT.write_text(content, encoding="ascii")
        print(f"wrote {len(SEEDS)} reference seeds to {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
