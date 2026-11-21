#!/usr/bin/env python3
"""Static inventory of the PyBot Python codebase (fase 0 do GOAL.md).

Walks every .py file in the repository (excluding .git, venvs and __pycache__) and
collects, without executing anything:

  inventory.json
    loc:            physical lines per directory
    imports:        top-level packages imported, split stdlib / local / third-party
    stdlib_detail:  how often each stdlib module is imported and which names are used
    regex:          summary of re.* usage + feature histogram (see regex_inventory.json)
    dynamism:       eval/exec/getattr/__import__/importlib/globals/setattr/decorators/...
    nondet:         random.*, time.*, datetime.now..., hash(), id(), set()/frozenset() sites

  regex_inventory.json
    every (file, line, pattern) literal found, tagged with the features it uses.

Usage: python3 tools/recon/inventory.py
"""
import ast
import collections
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXCLUDE_DIRS = {".git", ".venv", "node_modules", "__pycache__", ".github"}

PY_FILES = sorted(p for p in ROOT.rglob("*.py")
                  if not any(part in EXCLUDE_DIRS for part in p.relative_to(ROOT).parts))


def rel(p):
    return str(p.relative_to(ROOT))


def literal_str(node, env):
    """Best-effort literal evaluation of a pattern argument (str only)."""
    if node is None:
        return None
    if isinstance(node, ast.Constant):
        return node.value if isinstance(node.value, str) else None
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
        left = literal_str(node.left, env)
        right = literal_str(node.right, env)
        if left is not None and right is not None:
            return left + right
        return None
    if isinstance(node, ast.Name) and node.id in env:
        return env[node.id]
    if isinstance(node, ast.JoinedStr):
        out = []
        for part in node.values:
            if isinstance(part, ast.Constant) and isinstance(part.value, str):
                out.append(part.value)
            else:
                return None  # dynamic f-string: cannot know the pattern statically
        return "".join(out)
    if isinstance(node, ast.Subscript) and isinstance(node.value, ast.Name):
        # e.g. BODY = BODY[:-1] + ")"  — module constants rebuilt in place
        base = env.get(node.value.id)
        if base is None:
            return None
        sl = node.slice
        if isinstance(sl, ast.Slice):
            lower = literal_str(ast.Constant(sl.lower.value), env) if isinstance(sl.lower, ast.Constant) else None
            upper = literal_str(ast.Constant(sl.upper.value), env) if isinstance(sl.upper, ast.Constant) else None
            try:
                start = int(sl.lower.value) if sl.lower is not None and isinstance(sl.lower, ast.Constant) else None
                stop = int(sl.upper.value) if sl.upper is not None and isinstance(sl.upper, ast.Constant) else None
                return base[start:stop]
            except (TypeError, ValueError):
                return None
        if isinstance(sl, ast.Constant) and isinstance(sl.value, int):
            return base[sl.value]
        return None
    return None


def module_level_str_env(tree):
    """Module-level NAME = <literal string expr> bindings (handles concatenation)."""
    env = {}
    for node in tree.body:
        target = None
        value = None
        if isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
            target, value = node.targets[0].id, node.value
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name) and node.value is not None:
            target, value = node.target.id, node.value
        if target:
            s = literal_str(value, env)
            if s is not None:
                env[target] = s
        elif isinstance(node, ast.AugAssign) and isinstance(node.op, ast.Add) and isinstance(node.target, ast.Name):
            prev = env.get(node.target.id)
            rhs = literal_str(node.value, env)
            if prev is not None and rhs is not None:
                env[node.target.id] = prev + rhs
    return env


RE_FUNCS = {"compile", "match", "search", "fullmatch", "findall", "finditer",
            "sub", "subn", "split", "escape", "purge", "template", "error", "Pattern", "RegexFlag"}

FEATURE_CHECKS = [
    ("lookahead", re.compile(r"\(\?=|\(\?!")),
    ("lookbehind", re.compile(r"\(\?<=|\(\?<!")),
    ("named_group", re.compile(r"\(\?P<")),
    ("named_backref", re.compile(r"\(\?P=|\\g<[A-Za-z_]")),
    ("numbered_backref", re.compile(r"\\(?:[1-9]|[1-9][0-9])(?![0-9])")),
    ("lazy_quantifier", re.compile(r"(?:\*|\+|\?|\{[0-9]+(?:,[0-9]*)?\})\?")),
    ("possessive_like", re.compile(r"(?:\*|\+|\?|\{[0-9]+(?:,[0-9]*)?\})\+")),
    ("named_group_ref_syntax", re.compile(r"\(\?P=")),
    ("word_boundary", re.compile(r"\\[bB]")),
    ("abs_anchors", re.compile(r"\\[AZ]")),
    ("unicode_classes", re.compile(r"\\[wWdDsS]")),
    ("inline_flags", re.compile(r"\(\?[aiLmsux]+(?::|\))")),
    ("comment", re.compile(r"\(\?#")),
    ("atomic_group_unsupported", re.compile(r"\(\?>")),
    ("conditionals_unsupported", re.compile(r"\(\?\(")),
]

FLAG_NAMES = {"re.I", "re.IGNORECASE", "re.M", "re.MULTILINE", "re.S", "re.DOTALL",
              "re.X", "re.VERBOSE", "re.U", "re.UNICODE", "re.A", "re.ASCII",
              "re.L", "re.LOCALE", "re.DEBUG", "I", "M", "S", "X", "U", "A", "L"}


def flag_expr(node):
    """Render a flags argument like re.I or re.I | re.M as a sorted string."""
    parts = []
    stack = [node]
    while stack:
        cur = stack.pop()
        if isinstance(cur, ast.BinOp) and isinstance(cur.op, ast.BitOr):
            stack.extend([cur.left, cur.right])
        elif isinstance(cur, ast.Attribute):
            parts.append(cur.attr)
        elif isinstance(cur, ast.Name):
            parts.append(cur.id)
        elif isinstance(cur, ast.Constant) and isinstance(cur.value, int):
            parts.append(str(cur.value))
    return "|".join(sorted(set(parts)))


def test_compile_features(pattern):
    feats = [name for name, rx in FEATURE_CHECKS if rx.search(pattern)]
    try:
        re.compile(pattern)
    except Exception:
        feats.append("INVALID_IN_PY_RE")
    return feats


loc = collections.Counter()
loc_files = collections.Counter()
import_top = collections.Counter()
stdlib_names = collections.defaultdict(collections.Counter)   # module -> Counter(of imported names)
third_party_imports = collections.Counter()                    # dotted names
local_imports = collections.Counter()
regex_calls = collections.Counter()
regex_flags = collections.Counter()
patterns = []
unresolved_patterns = 0

DYNAMIC_CALLS = {
    "eval": "eval", "exec": "exec", "__import__": "__import__",
    "getattr": "getattr", "setattr": "setattr", "delattr": "delattr",
    "globals": "globals()", "locals": "locals()", "vars": "vars()",
    "open": "open()", "input": "input()", "breakpoint": "breakpoint()",
    "compile": "compile()", "getattr_static": "getattr_static()",
    "exit": "exit()", "quit": "quit()",
}
dynamism = collections.defaultdict(list)
decorators = collections.Counter()

NONDET = {
    "random": "random.*",
    "time": "time.*",
    "hash": "hash()",
    "id": "id()",
    "set": "set()",
    "frozenset": "frozenset()",
    "os.urandom": "os.urandom",
}
nondet = collections.defaultdict(list)
datetime_now_sites = []
set_iteration_sites = []
re_module_names = {"re"}

for path in PY_FILES:
    rel_path = rel(path)
    try:
        source = path.read_text(encoding="utf-8")
    except (UnicodeDecodeError, ValueError):
        source = path.read_text(encoding="utf-8", errors="replace")
    loc[str(Path(rel_path).parts[0]) if len(Path(rel_path).parts) > 1 else "."] += source.count("\n") + 1
    loc_files[str(Path(rel_path).parts[0]) if len(Path(rel_path).parts) > 1 else "."] += 1
    try:
        tree = ast.parse(source, filename=rel_path)
    except SyntaxError as exc:
        print(f"WARNING: could not parse {rel_path}: {exc}", file=sys.stderr)
        continue
    env = module_level_str_env(tree)

    for node in ast.walk(tree):
        # ---- imports
        if isinstance(node, ast.Import):
            for alias in node.names:
                top = alias.name.split(".")[0]
                import_top[top] += 1
                if top in sys.stdlib_module_names:
                    stdlib_names[top]["*"] += 1
                elif top == "bot":
                    local_imports[alias.name] += 1
                else:
                    third_party_imports[alias.name] += 1
        elif isinstance(node, ast.ImportFrom):
            if node.level:  # relative import
                import_top["bot (relative)"] += 1
                continue
            top = (node.module or "").split(".")[0]
            if not top:
                continue
            import_top[top] += 1
            if top in sys.stdlib_module_names:
                for alias in node.names:
                    stdlib_names[top][alias.name] += 1
            elif top == "bot":
                local_imports[node.module] += 1
            else:
                third_party_imports[node.module] += 1
        # ---- regex
        elif isinstance(node, ast.Call):
            func = node.func
            if isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name):
                base, attr = func.value.id, func.attr
                if base in re_module_names and attr in RE_FUNCS:
                    regex_calls[attr] += 1
                    # flags keyword/positional
                    flag_node = None
                    for kw in node.keywords:
                        if kw.arg == "flags":
                            flag_node = kw.value
                    if flag_node is None:
                        idx = {"compile": 1, "match": 2, "search": 2, "fullmatch": 2,
                               "findall": 2, "finditer": 2, "split": 2, "sub": 3, "subn": 3}.get(attr)
                        if idx is not None and len(node.args) > idx:
                            flag_node = node.args[idx]
                    if flag_node is not None:
                        regex_flags[flag_expr(flag_node)] += 1
                    # pattern extraction
                    if attr in ("compile", "match", "search", "fullmatch", "findall",
                                "finditer", "sub", "subn", "split"):
                        pnode = node.args[0] if node.args else None
                        pattern = literal_str(pnode, env) if pnode else None
                        if pattern is None and attr != "compile":
                            # match/search/etc may take a compiled pattern object as first arg — skip
                            if isinstance(pnode, ast.Call):
                                pattern = literal_str(pnode.args[0], env) if pnode.args else None
                        if pattern is not None:
                            patterns.append({
                                "file": rel_path, "line": node.lineno,
                                "func": attr, "pattern": pattern,
                                "flags": flag_expr(flag_node) if flag_node is not None else "",
                                "features": test_compile_features(pattern),
                            })
                        else:
                            unresolved_patterns += 1
                # ---- other dynamic calls via attribute on imported modules (light)
                dotted = f"{base}.{attr}"
                if dotted in NONDET or dotted.split(".")[0] in {"random", "time"} and base in {"random", "time"}:
                    nondet[dotted].append(f"{rel_path}:{node.lineno}")
                if dotted in ("datetime.now", "datetime.today", "datetime.utcnow",
                              "date.today", "datetime.fromtimestamp"):
                    datetime_now_sites.append(f"{rel_path}:{node.lineno}")
            elif isinstance(func, ast.Name):
                name = func.id
                if name in DYNAMIC_CALLS:
                    dynamism[DYNAMIC_CALLS[name]].append(f"{rel_path}:{node.lineno}")
                if name in ("hash", "id", "set", "frozenset"):
                    nondet[NONDET[name]].append(f"{rel_path}:{node.lineno}")
                if name in ("random", "time"):  # unlikely but harmless
                    nondet[name].append(f"{rel_path}:{node.lineno}")
        # ---- random/time via module attributes (import random; random.choice)
        elif isinstance(node, ast.Attribute):
            if isinstance(node.value, ast.Name) and node.value.id in ("random", "time"):
                pass  # counted at the Call level; attribute alone is fine
        # ---- decorators
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            for dec in node.decorator_list:
                if isinstance(dec, ast.Name):
                    decorators[dec.id] += 1
                elif isinstance(dec, ast.Attribute):
                    decorators[dec.attr] += 1
                elif isinstance(dec, ast.Call):
                    tgt = dec.func
                    if isinstance(tgt, ast.Name):
                        decorators[tgt.id] += 1
                    elif isinstance(tgt, ast.Attribute):
                        decorators[tgt.attr] += 1
        # ---- datetime imported as `from datetime import datetime` then datetime.now()
    for node in ast.walk(tree):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
            attr = node.func.attr
            if attr in ("now", "today", "utcnow") and isinstance(node.func.value, ast.Name):
                # crude: value name looks like a datetime class
                if node.func.value.id in ("datetime", "date", "_datetime"):
                    site = f"{rel_path}:{node.lineno}"
                    if site not in datetime_now_sites:
                        datetime_now_sites.append(site)
        # ---- for x in {literal set} / for x in {a, b}:
        elif isinstance(node, ast.For):
            if isinstance(node.iter, ast.Set):
                set_iteration_sites.append(f"{rel_path}:{node.lineno}")

# set() call sites are in nondet['set()']; also capture bare `set` name iteration via `in set()`
regexp_feature_totals = collections.Counter()
for p in patterns:
    regexp_feature_totals.update(p["features"])

stdlib_top = {m: sum(stdlib_names[m].values()) for m in stdlib_names}
out = {
    "files": len(PY_FILES),
    "loc_by_top_dir": dict(sorted(loc.items())),
    "files_by_top_dir": dict(sorted(loc_files.items())),
    "imports_top": dict(import_top.most_common()),
    "stdlib_total": dict(sorted(stdlib_top.items(), key=lambda kv: -kv[1])),
    "stdlib_names": {m: dict(c.most_common()) for m, c in sorted(stdlib_names.items())},
    "third_party": dict(third_party_imports.most_common()),
    "local_imports_top": dict(local_imports.most_common(30)),
    "regex_calls": dict(regex_calls.most_common()),
    "regex_flags_used": dict(regex_flags.most_common()),
    "regex_patterns_total": len(patterns),
    "regex_patterns_unresolved": unresolved_patterns,
    "regex_feature_totals": dict(regexp_feature_totals.most_common()),
    "dynamic_calls": {k: {"count": len(v), "sites": v[:40]} for k, v in sorted(dynamism.items())},
    "dynamic_calls_total": len(dynamism),
    "decorators": dict(decorators.most_common(40)),
    "nondeterminism": {k: {"count": len(v), "sites": v[:60]} for k, v in sorted(nondet.items())},
    "datetime_sites": datetime_now_sites,
    "set_iteration_sites": set_iteration_sites,
}

inventory_path = ROOT / "tools" / "recon" / "inventory.json"
inventory_path.write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

regex_path = ROOT / "tools" / "recon" / "regex_inventory.json"
unique = {}
for p in patterns:
    key = (p["pattern"], p["flags"])
    entry = unique.get(key)
    if entry is None:
        unique[key] = {"pattern": p["pattern"], "flags": p["flags"], "count": 1,
                       "first_seen": f"{p['file']}:{p['line']}",
                       "funcs": [p["func"]], "features": p["features"]}
    else:
        entry["count"] += 1
        if p["func"] not in entry["funcs"]:
            entry["funcs"].append(p["func"])
unique_patterns = sorted(unique.values(), key=lambda e: -e["count"])
regex_doc = {
    "calls": out["regex_calls"],
    "flags_used": out["regex_flags_used"],
    "feature_totals": out["regex_feature_totals"],
    "unresolved_patterns": unresolved_patterns,
    "pattern_call_sites": len(patterns),
    "unique_patterns": len(unique_patterns),
    "patterns": unique_patterns,
}
regex_path.write_text(json.dumps(regex_doc, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")

print(f"files scanned: {len(PY_FILES)}")
print(f"inventory written: {inventory_path}")
print(f"regex inventory written: {regex_path} ({len(patterns)} patterns, {unresolved_patterns} unresolved)")
print(f"stdlib top: {out['stdlib_total']}")
print(f"third-party: {out['third_party']}")
