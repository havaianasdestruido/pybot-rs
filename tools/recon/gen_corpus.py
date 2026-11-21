#!/usr/bin/env python3
"""Generate the golden corpus (fase 0 do GOAL.md).

Runs the original Python PyBot over a large, deterministic battery of inputs and
records, for every turn: the raw reply, the winning module, the candidate modules,
a compact view of the relevant memory, and a sha1 digest of the *full* memory.

Determinism: PYTHONHASHSEED must be 0 (the wrapper exports it) and every session is
seeded with corpus_seed * 100000 + session_index before its first turn.

Usage:
    PYTHONHASHSEED=0 python3 tools/recon/gen_corpus.py --out tests/golden/corpus.jsonl --workers 2

Output: one JSON object per line (one per turn). Metadata goes to the manifest
written next to the corpus (see --manifest).
"""
import argparse
import hashlib
import json
import multiprocessing
import random
import sys
from pathlib import Path

CORPUS_SEED = 20261121

# ---------------------------------------------------------------- seed banks

GREETINGS_AND_SMALL_TALK = [
    "hello", "hi", "hey there", "good morning", "good afternoon", "good evening",
    "how are you?", "how are you doing", "what's up", "yo", "hiya",
    "hello, nice to meet you", "good evening! how are you",
    "i'm doing well, thanks", "not great today", "i feel amazing",
    "thanks", "thank you so much", "cheers", "bye", "goodbye", "see you later",
    "you're welcome", "that's helpful, thanks", "no problem",
]

SELF_INTRO = [
    "my name is {name}", "hi, i'm {name}", "hello my name is {name}",
    "call me {name}", "i'm {name}, nice to meet you",
]

RECALL_NAME = [
    "what's my name?", "what is my name", "do you know my name?",
    "who am i?", "tell me my name", "remind me of my name",
]

NAMES = ["Sam", "Priya", "Marco", "Lucia", "Tom", "Aisha", "Elena", "João", "Beatriz", "Kofi"]

ADVICE_QUESTIONS = [
    "how long should i boil an egg",
    "my kettle is full of limescale, how do i get rid of it",
    "why do onions make me cry",
    "how do i get rid of red wine stains",
    "how much detergent should i use in the washing machine",
    "how do i fall asleep faster",
    "how much caffeine is too much",
    "what can i do for a common cold",
    "how do i fix a slow laptop",
    "why won't my wifi connect",
    "how do i change a flat tyre",
    "how often should i change the oil in my car",
    "how do i save money on groceries",
    "how do i cook rice without a rice cooker",
    "how long does pasta take to cook",
    "what temperature should chicken be cooked to",
    "how do i stop procrastinating",
    "how do i study for an exam effectively",
    "how much water should i drink a day",
    "how do i start running as a beginner",
    "what can i do about a sunburn",
    "how do i remove a splinter",
    "what should i do about a nosebleed",
    "how do i train a puppy not to bark",
    "how often should i water my tomato plants",
    "why are my houseplant leaves turning yellow",
    "how do i make crispy roast potatoes",
    "what's the ratio for pancake batter",
    "how do i organize my fridge",
    "how do i get rid of mould in the bathroom",
    "why does my smoke alarm keep beeping",
    "how do i unblock a sink",
    "how do i write a good cv",
    "how do i ask for a pay rise",
    "what's the best way to learn piano",
    "how do i remember names better",
    "how do i deal with a jet lag",
    "how do i keep bananas from ripening too fast",
    "how do i clean a cast iron pan",
]

FACTUAL_QUESTIONS = [
    "what is the capital of france",
    "what is the capital of japan",
    "why is the sky blue",
    "what is photosynthesis",
    "how far away is the moon",
    "who wrote hamlet",
    "what is the speed of light",
    "what is the boiling point of water",
    "tell me about the second world war",
    "what is the largest planet in the solar system",
    "who painted the mona lisa",
    "what is the tallest mountain on earth",
    "how many bones are in the human body",
    "what year did world war 2 end",
    "what is the chemical symbol for gold",
    "how many continents are there",
    "what is the largest ocean on earth",
    "who discovered penicillin",
    "what is the freezing point of water",
    "how many days are there in a leap year",
]

FEELINGS_AND_LIFE = [
    "i feel sad today", "i'm stressed about work", "i'm really anxious lately",
    "i'm so happy right now", "i'm bored", "i feel lonely",
    "i'm proud of myself", "i'm angry at my brother", "i'm nervous about the interview tomorrow",
    "i can't sleep", "i've had a hard week", "today was a good day",
    "i don't know what to do with my life", "i'm overwhelmed",
]

META_AND_CHITCHAT = [
    "who are you?", "what are you?", "what can you do?", "tell me a joke", "tell me something interesting",
    "do you have feelings?", "are you a robot?", "how old are you?", "what's your name?",
    "tell me a story", "say something nice", "do you dream?", "what do you think about ai?",
    "i like music", "i love cooking", "my favourite colour is blue", "i have two kids",
    "i work in an office", "i'm retired", "i live in brazil",
]

EDGE_CASES = [
    "", "   ", "?", "!", "???", "!!!", ".", "...", "a", "ok", "yes", "no", "maybe",
    "WHAT IS MY NAME", "What Is My Name?", "hElLo tHeRe",
    "whats my nme", "hw are u", "wat is the captal of france",
    "a" * 400,
    "word " * 200,
    "no " * 80,
    "😀😀😀", "🎉 hello 🎉", "𝕳𝖊𝖑𝖑𝖔 there",
    "olá, tudo bem?", "qual é a capital do brasil?", "quero um café forte",
    "SELECT * FROM users;", "<script>alert(1)</script>", "{{7*7}}", "%s %s %s",
    "line one\nline two", "col1\tcol2", "\x00\x01",
    "i\\nsaid\\nno", "\\b\\w+", "(?=lookahead)", "\\1\\2",
    "naïve café résumé", "Ångström", "œuvre",
]

# Scripted multi-turn conversations (the workhorse of the corpus: memory-heavy).
SCRIPTED_SESSIONS = [
    ["hi, i'm Sam", "what's my name?", "how long should i boil an egg", "and for a runny yolk?", "how long does that take"],
    ["my name is Priya", "how are you?", "what is my name?", "tell me a joke", "thanks"],
    ["hello", "i feel sad today", "why?", "what can i do about it?", "bye"],
    ["what is the capital of france", "and what about germany?", "why is the sky blue", "tell me more", "who wrote hamlet"],
    ["how long should i boil an egg", "how long does that take", "what temperature should chicken be cooked to", "and how long does pasta take", "thanks, that helps"],
    ["my kettle is full of limescale, how do i get rid of it", "how often should i do it", "what about my washing machine"],
    ["i'm nervous about the interview tomorrow", "how do i prepare", "what if i fail", "thanks"],
    ["who are you?", "what can you do?", "tell me about yourself", "do you have feelings?"],
    ["why do onions make me cry", "how do i avoid it", "what else makes me cry"],
    ["i have two kids", "how do i fall asleep faster", "i can't sleep", "how much caffeine is too much"],
    ["what is photosynthesis", "why is the sky blue", "how far away is the moon", "what is the speed of light"],
    ["how do i fix a slow laptop", "why won't my wifi connect", "how do i change a flat tyre"],
    ["hi", "hi", "hi", "are you ignoring me?", "hello again"],
    ["tell me a joke", "another one", "another", "okay, enough"],
    ["i like music", "my favourite colour is blue", "i work in an office", "what do you know about me?"],
    ["how do i save money on groceries", "how do i ask for a pay rise", "what's the best way to learn piano"],
    ["what should i do about a sunburn", "how do i remove a splinter", "what about a nosebleed"],
    ["i'm angry at my brother", "he ate the last yoghurt", "what should i do"],
    ["what is the largest planet", "how far away is the moon", "what year did world war 2 end"],
    ["how do i keep bananas from ripening too fast", "how do i organize my fridge", "how do i cook rice without a rice cooker"],
    ["tell me everything", "why?", "what do you mean", "explain it more simply"],
    ["yes", "no", "maybe", "i don't know"],
    ["how much water should i drink a day", "is coffee dehydrating", "what about tea"],
    ["how do i start running as a beginner", "how often should i run", "what about knee pain"],
    ["my name is Marco", "what's my name?", "my name is Tom", "what's my name now?"],
    ["what's the boiling point of water", "and the freezing point", "what is the chemical symbol for gold"],
    ["who painted the mona lisa", "who wrote hamlet", "what is the tallest mountain on earth"],
    ["how do i train a puppy not to bark", "how often should i water my tomato plants", "why are my houseplant leaves turning yellow"],
    ["it", "that", "the one from before", "tell me more"],
    ["same question", "same again", "and now?", "go on"],
    ["how do i unblock a sink", "how do i get rid of mould in the bathroom", "why does my smoke alarm keep beeping"],
    ["i'm retired", "i live in brazil", "what can you tell me about me?"],
    ["good morning", "good morning again", "good evening"],
    ["how do i remember names better", "how do i write a good cv", "how do i stop procrastinating"],
    ["what is the boiling point of water", "what is the freezing point of water", "how many days in a leap year"],
    ["why is the sky blue", "i never knew that", "how does that work"],
    ["how do i bake bread", "what temperature should chicken be cooked to", "how long does pasta take to cook"],
    ["do you dream?", "what do you think about ai?", "are you a robot?"],
    ["i feel lonely", "nobody understands me", "what should i do"],
    ["remind me what you said", "say that again", "can you repeat that"],
]

# Template-generated multi-turn conversations.
TEMPLATE_SESSIONS = []

for _name in NAMES[:8]:
    TEMPLATE_SESSIONS.append([f"hi, i'm {_name}", "what's my name?"])
for _name in NAMES[:8]:
    TEMPLATE_SESSIONS.append([f"my name is {_name}", "how are you?", "what is my name?", "bye"])
for _t in ["boiling an egg", "limescale in my kettle", "red wine stains", "falling asleep",
           "wifi problems", "a slow laptop", "saving money", "my tomato plants"]:
    TEMPLATE_SESSIONS.append([f"tell me about {_t}", "why?", "tell me more", "how long does that take", "thanks"])
for _n in range(4):
    TEMPLATE_SESSIONS.append(["tell me a joke", "another", "one more", "okay stop"])
TEMPLATE_SESSIONS.append(["i feel sad today", "i don't know why", "what should i do", "thanks for listening"])
TEMPLATE_SESSIONS.append(["i'm stressed about work", "my boss is difficult", "what can i do", "thanks"])
TEMPLATE_SESSIONS.append(["hello", "what are you?", "what can you do?", "my name is Sam", "what's my name?", "bye"])

# Knowledge-mined probes: one question per Entry of the knowledge base, round-robin across
# domains, so the corpus exercises the entry matcher and the topic arbitration instead of only
# the hand-written prompts above. The probe phrase is the first leaf of each key group; the
# template rotates through four shapes. Probes are not meant to read naturally - they are
# signals for the matcher - but each one lands somewhere definite in the Python bot.
_PROBE_TEMPLATES = ("tell me about {p}", "how do i {p}", "what do you know about {p}",
                    "what is {p}")


def _first_leaf(item):
    if isinstance(item, str):
        return item.strip()
    for sub in item:
        leaf = _first_leaf(sub)
        if leaf:
            return leaf
    return ""


def knowledge_probe_sessions(limit):
    from bot.knowledge import entries
    by_domain = {}
    for entry_obj in entries():
        by_domain.setdefault(getattr(entry_obj, "domain", ""), []).append(entry_obj)
    pools = [iter(by_domain[d]) for d in sorted(by_domain)]
    probes = []
    while len(probes) < limit and pools:
        for pool in list(pools):
            if len(probes) >= limit:
                break
            try:
                entry_obj = next(pool)
            except StopIteration:
                pools.remove(pool)
                continue
            phrase = " ".join(leaf for leaf in (_first_leaf(item) for item in entry_obj.key)
                              if leaf).strip()
            phrase = " ".join(phrase.split())[:60]
            if len(phrase) < 3 or not any(ch.isalpha() for ch in phrase):
                continue
            template = _PROBE_TEMPLATES[len(probes) % len(_PROBE_TEMPLATES)]
            probes.append([template.format(p=phrase)])
    return probes


def build_sessions():
    """Returns [(session_id, [turn_inputs])]."""
    sessions = []

    def add(turns):
        sessions.append(turns)

    for text in GREETINGS_AND_SMALL_TALK:
        add([text])
    for text in ADVICE_QUESTIONS:
        add([text])
    for text in FACTUAL_QUESTIONS:
        add([text])
    for text in FEELINGS_AND_LIFE:
        add([text])
    for text in META_AND_CHITCHAT:
        add([text])
    for text in EDGE_CASES:
        add([text])
    for name in NAMES:
        for tpl in SELF_INTRO:
            add([tpl.format(name=name)])
    for name in NAMES:
        add([f"hi, i'm {name}", "what's my name?"])
    for text in RECALL_NAME:
        add(["my name is Tom", text])
    for scripted in SCRIPTED_SESSIONS:
        add(list(scripted))
    for templated in TEMPLATE_SESSIONS:
        add(list(templated))
    # Repetition / frequency handling inside one session
    add(["tell me a joke", "tell me a joke", "tell me a joke", "tell me a joke"])
    add(["how long should i boil an egg", "how long should i boil an egg"])
    add(["hello", "hello", "hello"])
    return sessions


# ---------------------------------------------------------------- fuzz

_FUZZ_BASE = ADVICE_QUESTIONS + FACTUAL_QUESTIONS + GREETINGS_AND_SMALL_TALK + META_AND_CHITCHAT
_ACCENTS = "áàâãéèêíìîóòôõúùûç"
_VOWELS = "aeiou"


def fuzz_prompt(rng):
    base = rng.choice(_FUZZ_BASE)
    text = base
    for _ in range(rng.randint(1, 3)):
        edit = rng.choice(["del", "dup", "swap", "case", "ins_char", "subs_char",
                           "accent", "digit", "drop_vowel", "space"])
        if not text:
            break
        i = rng.randrange(len(text))
        if edit == "del":
            text = text[:i] + text[i + 1:]
        elif edit == "dup":
            text = text[:i] + text[i] + text[i:]
        elif edit == "swap" and i + 1 < len(text):
            text = text[:i] + text[i + 1] + text[i] + text[i + 2:]
        elif edit == "case":
            text = text[:i] + text[i].swapcase() + text[i + 1:]
        elif edit == "ins_char":
            text = text[:i] + rng.choice("abcdefghijklmnopqrstuvwxyz") + text[i:]
        elif edit == "subs_char":
            text = text[:i] + rng.choice("abcdefghijklmnopqrstuvwxyz") + text[i + 1:]
        elif edit == "accent":
            vowels = [j for j, c in enumerate(text) if c in _VOWELS]
            if vowels:
                j = rng.choice(vowels)
                text = text[:j] + rng.choice(_ACCENTS) + text[j + 1:]
        elif edit == "digit":
            text = text[:i] + rng.choice("0123456789") + text[i:]
        elif edit == "drop_vowel":
            vowels = [j for j, c in enumerate(text) if c in _VOWELS]
            if vowels:
                j = rng.choice(vowels)
                text = text[:j] + text[j + 1:]
        elif edit == "space":
            text = text[:i] + " " + text[i:]
    return text


def noise_prompt(rng):
    n = rng.randint(1, 60)
    alphabet = "abcdefghijklmnopqrstuvwxyz .,!?'\"-"
    return "".join(rng.choice(alphabet) for _ in range(n))


# ---------------------------------------------------------------- memory & digests

DROP_MEMORY_KEYS = {"turns", "understood"}
MAX_VALUE_CHARS = 400


def _canonical(obj):
    """JSON-serializable canonical form (sets sorted by repr; unknown types -> repr)."""
    if isinstance(obj, dict):
        return {str(k): _canonical(obj[k]) for k in sorted(obj, key=lambda k: str(k))}
    if isinstance(obj, (list, tuple)):
        return [_canonical(x) for x in obj]
    if isinstance(obj, (set, frozenset)):
        return sorted((_canonical(x) for x in obj), key=repr)
    if obj is None or isinstance(obj, (bool, int, float, str)):
        return obj
    return repr(obj)


def memory_digest(memory):
    blob = json.dumps(_canonical(memory), ensure_ascii=False, sort_keys=True,
                      separators=(",", ":"), default=repr)
    return hashlib.sha1(blob.encode("utf-8", "surrogatepass")).hexdigest()


def relevant_memory(memory):
    out = {}
    for key, value in memory.items():
        if key in DROP_MEMORY_KEYS:
            continue
        try:
            blob = json.dumps(_canonical(value), ensure_ascii=False, sort_keys=True, default=repr)
        except Exception:  # noqa: BLE001 - corpus generation must not die on one weird value
            blob = repr(value)
        if len(blob) > MAX_VALUE_CHARS:
            out[key] = {"__truncated__": True,
                        "sha1": hashlib.sha1(blob.encode("utf-8", "surrogatepass")).hexdigest(),
                        "chars": len(blob)}
        else:
            try:
                out[key] = json.loads(blob)
            except Exception:  # noqa: BLE001
                out[key] = blob
    return out


# ---------------------------------------------------------------- worker

_RUNTIME = None


def _runtime():
    global _RUNTIME
    if _RUNTIME is None:
        from bot import runtime
        _RUNTIME = runtime
    return _RUNTIME


def run_session(spec):
    session_id, turn_inputs = spec
    runtime = _runtime()
    index = int(session_id[1:])
    random.seed(CORPUS_SEED * 100000 + index)
    memory = {}
    rows = []
    for turn_index, text in enumerate(turn_inputs):
        result = runtime.turn(text, memory)
        raw_reply = result.get("reply")
        reply = " ".join(str(raw_reply).split()) if raw_reply is not None else ""
        candidates = [{"module": c.get("module"), "priority": c.get("priority")}
                      for c in result.get("candidates", [])]
        rows.append({
            "session": session_id,
            "turn": turn_index,
            "input": text,
            "reply": reply,
            "module": result.get("module"),
            "candidates": candidates,
            "memory_keys": sorted(memory.keys()),
            "memory_digest": memory_digest(memory),
            "memory": relevant_memory(memory),
        })
    return rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--workers", type=int, default=2)
    parser.add_argument("--fuzz", type=int, default=1500, help="number of fuzz single-turn sessions")
    parser.add_argument("--noise", type=int, default=150, help="number of pure-noise sessions")
    parser.add_argument("--probes", type=int, default=900, help="number of knowledge-mined probe sessions")
    args = parser.parse_args()
    # The bot lives in the repository root; make it importable no matter how we were called.
    repo_root = Path(__file__).resolve().parents[2]
    if str(repo_root) not in sys.path:
        sys.path.insert(0, str(repo_root))

    import os
    if os.environ.get("PYTHONHASHSEED") != "0":
        print("error: run with PYTHONHASHSEED=0 for a reproducible corpus", file=sys.stderr)
        return 2

    base = build_sessions()
    rng = random.Random(CORPUS_SEED)
    for _ in range(args.fuzz):
        base.append([fuzz_prompt(rng)])
    for _ in range(args.noise):
        base.append([noise_prompt(rng)])
    if args.probes > 0:
        # appended last so the ids of the curated/fuzz/noise sessions stay stable
        base.extend(knowledge_probe_sessions(args.probes))

    sessions = [(f"s{i:04d}", turns) for i, turns in enumerate(base)]
    n_turns = sum(len(s[1]) for s in sessions)
    print(f"planning {len(sessions)} sessions, {n_turns} turns, workers={args.workers}", flush=True)

    rows = []
    if args.workers > 1:
        ctx = multiprocessing.get_context("fork")
        with ctx.Pool(args.workers) as pool:
            for i, session_rows in enumerate(pool.imap(run_session, sessions, chunksize=8)):
                rows.extend(session_rows)
                if (i + 1) % 50 == 0:
                    print(f"  {i + 1}/{len(sessions)} sessions done, {len(rows)} turns", flush=True)
    else:
        for i, spec in enumerate(sessions):
            rows.extend(run_session(spec))
            if (i + 1) % 50 == 0:
                print(f"  {i + 1}/{len(sessions)} sessions done, {len(rows)} turns", flush=True)

    rows.sort(key=lambda r: (r["session"], r["turn"]))
    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    blob = "".join(json.dumps(r, ensure_ascii=False, separators=(",", ":")) + "\n" for r in rows)
    out_path.write_text(blob, encoding="utf-8")

    manifest = {
        "version": 1,
        "created_by": "tools/recon/gen_corpus.py",
        "corpus_seed": CORPUS_SEED,
        "pythonhashseed": "0",
        "python": sys.version.split()[0],
        "sessions": len(sessions),
        "turns": len(rows),
        "corpus_sha256": hashlib.sha256(blob.encode("utf-8")).hexdigest(),
        "schema": {
            "session": "sNNNN, stable per corpus version",
            "turn": "0-based turn index inside the session",
            "input": "exact text fed to runtime.turn",
            "reply": "runtime reply, normalized with ' '.join(str(reply).split()) as in pybot.py",
            "module": "winning module name (None when no module proposed / fallback)",
            "candidates": "[{module, priority}] in arbiter order (replies omitted to keep the file small)",
            "memory_keys": "sorted keys of the post-turn memory",
            "memory_digest": "sha1 of the canonical JSON of the FULL post-turn memory",
            "memory": "post-turn memory minus turns/ and understood/, values truncated to 400 chars",
        },
    }
    manifest_path = Path(args.manifest)
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {out_path} ({len(rows)} turns, sha256 {manifest['corpus_sha256'][:16]}...)")
    print(f"wrote {manifest_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
