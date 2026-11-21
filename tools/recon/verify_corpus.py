#!/usr/bin/env python3
"""Spot-check the golden corpus: re-run a sample of sessions in a fresh process and
compare against the recorded JSONL, field by field (reply, module, candidates,
memory digest). Fails loudly on any mismatch.

Usage:
    PYTHONHASHSEED=0 python3 tools/recon/verify_corpus.py tests/golden/corpus.jsonl --sample 24
"""
import argparse
import json
import random
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

import tools.recon.gen_corpus as g  # noqa: E402  (shares the digest/memory helpers)


def rerun(session_id, turn_inputs):
    from bot import runtime
    index = int(session_id[1:])
    random.seed(g.CORPUS_SEED * 100000 + index)
    memory = {}
    rows = []
    for turn_index, text in enumerate(turn_inputs):
        result = runtime.turn(text, memory)
        reply = " ".join(str(result.get("reply")).split())
        rows.append({
            "session": session_id,
            "turn": turn_index,
            "input": text,
            "reply": reply,
            "module": result.get("module"),
            "candidates": [{"module": c.get("module"), "priority": c.get("priority")}
                           for c in result.get("candidates", [])],
            "memory_keys": sorted(memory.keys()),
            "memory_digest": g.memory_digest(memory),
        })
    return rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("corpus")
    parser.add_argument("--sample", type=int, default=24)
    parser.add_argument("--seed", type=int, default=7)
    args = parser.parse_args()

    recorded = {}
    with open(args.corpus, encoding="utf-8") as fh:
        for line in fh:
            row = json.loads(line)
            recorded.setdefault(row["session"], []).append(row)

    rng = random.Random(args.seed)
    session_ids = sorted(recorded)
    # stratified: prefer sessions that have multiple turns plus some single-turn ones
    multi = [s for s in session_ids if len(recorded[s]) > 2]
    single = [s for s in session_ids if len(recorded[s]) == 1]
    rng.shuffle(multi)
    rng.shuffle(single)
    picked = multi[: args.sample // 2] + single[: args.sample - args.sample // 2]

    mismatches = 0
    for session_id in picked:
        fresh = rerun(session_id, [r["input"] for r in recorded[session_id]])
        for old, new in zip(recorded[session_id], fresh):
            for field in ("reply", "module", "candidates", "memory_keys", "memory_digest"):
                if old[field] != new[field]:
                    mismatches += 1
                    print(f"MISMATCH {session_id} turn {old['turn']} field {field}")
                    print(f"   recorded: {json.dumps(old[field], ensure_ascii=False)[:200]}")
                    print(f"   fresh:    {json.dumps(new[field], ensure_ascii=False)[:200]}")
    print(f"checked {len(picked)} sessions ({sum(len(recorded[s]) for s in picked)} turns); "
          f"mismatches: {mismatches}")
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
