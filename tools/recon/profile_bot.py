#!/usr/bin/env python3
"""Profile the original Python PyBot (fase 0 do GOAL.md).

Three measurements, written to tools/recon/profile.json:
  startup:   import time of `bot` (a fresh subprocess measures it cleanly when
             --measure-startup is passed, see below)
  turns:     p50/p95/p99/mean per turn over a fixed battery (no profiler attached)
  cprofile:  aggregate where the time goes, over a smaller battery
  memory:    RSS after import and after the battery (resource module)

Usage:
    PYTHONHASHSEED=0 python3 tools/recon/profile_bot.py            # everything in-process
    PYTHONHASHSEED=0 python3 tools/recon/profile_bot.py --measure-startup   # fresh subprocess
"""
import argparse
import json
import random
import resource
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

# a fixed battery, representative of the corpus mix
BATTERY = [
    "hello",
    "hi, i'm Sam",
    "what's my name?",
    "how long should i boil an egg",
    "my kettle is full of limescale, how do i get rid of it",
    "why do onions make me cry",
    "tell me about radiator clatter",
    "how do i cant afford holiday",
    "what do you know about elephant live",
    "i feel sad today",
    "how do i fix a slow laptop",
    "what is the capital of france",
    "tell me a joke",
    "tell me about the second world war",
    "how do i fall asleep faster",
    "how do i save money on groceries",
    "what temperature should chicken be cooked to",
    "tell me about puncture fix",
    "how do i start a business how do i",
    "i'm nervous about the interview tomorrow",
]

CPROFILE_BATTERY = BATTERY[:12]


def rss_mb():
    return round(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024, 1)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--measure-startup", action="store_true",
                        help="spawn a fresh subprocess that prints its own import time")
    parser.add_argument("--out", default=str(ROOT / "tools" / "recon" / "profile.json"))
    args = parser.parse_args()

    report = {}

    if args.measure_startup:
        code = ("import time; t=time.perf_counter(); import bot.runtime as r; "
                "print(round(time.perf_counter()-t, 3))")
        times = []
        for _ in range(3):
            out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True,
                                 cwd=ROOT, env={"PYTHONHASHSEED": "0", "PATH": "/usr/bin:/bin"})
            times.append(float(out.stdout.strip()))
        report["startup_import_seconds"] = {"runs": times, "min": min(times)}
        Path(args.out).write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(report)
        return 0

    t0 = time.perf_counter()
    from bot import runtime
    import_seconds = time.perf_counter() - t0
    report["startup_import_seconds_in_process"] = round(import_seconds, 3)
    rss_after_import = rss_mb()
    report["rss_mb_after_import"] = rss_after_import

    # per-turn latency, no profiler attached — but the battery shares one growing memory,
    # so run each prompt in a fresh memory 3 times and take the median per prompt
    samples = []
    for text in BATTERY:
        per_prompt = []
        for seed in (11, 22, 33):
            random.seed(seed)
            memory = {}
            start = time.perf_counter()
            runtime.turn(text, memory)
            per_prompt.append(time.perf_counter() - start)
        samples.append({"input": text, "median_seconds": sorted(per_prompt)[1]})
    flat = sorted(s["median_seconds"] for s in samples)
    n = len(flat)

    def pct(p):
        k = max(0, min(n - 1, int(round((p / 100) * (n - 1)))))
        return round(flat[k], 3)

    report["turn_seconds"] = {
        "n_prompts": n,
        "mean": round(sum(flat) / n, 3),
        "p50": pct(50),
        "p95": pct(95),
        "p99": pct(99),
        "max": round(flat[-1], 3),
        "per_prompt": samples,
    }

    # RSS after a long-ish session of turns
    memory = {}
    random.seed(1234)
    for text in BATTERY * 5:
        runtime.turn(text, memory)
    report["rss_mb_after_100_turns"] = rss_mb()

    import cProfile
    import pstats
    import io

    def run_battery():
        for text in CPROFILE_BATTERY:
            memory = {}
            random.seed(99)
            runtime.turn(text, memory)

    profiler = cProfile.Profile()
    profiler.enable()
    run_battery()
    profiler.disable()
    stream = io.StringIO()
    stats = pstats.Stats(profiler, stream=stream).sort_stats("cumulative")
    stats.print_stats(45)
    report["cprofile_cumulative_top45"] = stream.getvalue()

    stream = io.StringIO()
    stats = pstats.Stats(profiler, stream=stream).sort_stats("tottime")
    stats.print_stats(45)
    report["cprofile_tottime_top45"] = stream.getvalue()

    out_path = Path(args.out)
    out_path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n",
                        encoding="utf-8")
    print(f"startup import: {report['startup_import_seconds_in_process']}s")
    print(f"rss after import: {report['rss_mb_after_import']} MB, "
          f"after 100 turns: {report['rss_mb_after_100_turns']} MB")
    t = report["turn_seconds"]
    print(f"turn seconds: mean {t['mean']} p50 {t['p50']} p95 {t['p95']} p99 {t['p99']} max {t['max']}")
    print(f"wrote {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
