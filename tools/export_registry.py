#!/usr/bin/env python3
"""Regenerate the Rust module registry from the reference bot.

`src/modules/registry.rs` is a copy of `bot/modules/` metadata: every module's `PRIORITY`
constant and the names of the hooks it defines (`observe`, `respond`, `after`, `choose`). This
script produces that copy, so the table is never edited by hand:

    python3 tools/export_registry.py > src/modules/registry.rs.part   # inspect
    python3 tools/export_registry.py --write                          # update in place

What it does
------------
1. Reads every `bot/modules/*.py` (skipping `__init__.py`).
2. Parses `PRIORITY = <int|float>` from the module source, defaulting to `0` — exactly what
   `runtime.load_modules()` does with `getattr(module, "PRIORITY", 0)`.
3. Parses the top-level `def observe/respond/after/choose` names, which is what
   `runtime._hooks()` finds with `getattr(module, name, None)`.
4. Sorts by `(-priority, name)` — the order `load_modules()` sorts by, and the reason "first
   candidate wins" is the same as "lowest index wins".
5. Emits `MODULES`, `NAME_INDEX` (sorted name -> index) and the hook-count constants.

The output is deterministic: same inputs, same bytes. `tests/registry.rs` re-derives the same
table from the Python sources at test time, so a stale export fails CI rather than lingering.

Only the Python standard library is used (this is a development tool, not runtime code).
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

HOOKS = ("after", "choose", "observe", "respond")

#: `PRIORITY = 9000  # the safety band…` and `PRIORITY = -999.5` both parse.
PRIORITY_RE = re.compile(r"^PRIORITY\s*=\s*(-?\d+(?:\.\d+)?)", re.MULTILINE)


def module_metadata(path: pathlib.Path) -> tuple[str, float | int, list[str]]:
    """(name, priority, hooks) for one module file."""
    source = path.read_text(encoding="utf-8")
    match = PRIORITY_RE.search(source)
    if match is None:
        priority: float | int = 0
    else:
        raw = match.group(1)
        priority = int(raw) if "." not in raw else float(raw)
    hooks = [hook for hook in HOOKS if re.search(rf"^def {hook}\(", source, re.MULTILINE)]
    return path.stem, priority, hooks


def load(modules_dir: pathlib.Path) -> list[tuple[str, float | int, list[str]]]:
    rows = []
    for path in sorted(modules_dir.glob("*.py")):
        if path.name == "__init__.py":
            continue
        rows.append(module_metadata(path))
    rows.sort(key=lambda row: (-row[1], row[0]))
    return rows


def priority_literal(value: float | int) -> str:
    if isinstance(value, int):
        return f"Priority::Int({value})"
    return f"Priority::Float({value!r})"


def hooks_literal(hooks: list[str]) -> str:
    if not hooks:
        return "&[]"
    return "&[" + ", ".join(f"Hook::{hook.capitalize()}" for hook in hooks) + "]"


def render(rows: list[tuple[str, float | int, list[str]]]) -> str:
    names = sorted((name, index) for index, (name, _, _) in enumerate(rows))
    width = max(len(name) for name, _ in names)
    index_width = max(len(str(index)) for _, index in names)

    # The doc comment above `MODULES` is hand-written and stays: only the table body and the
    # index are generated, so `--write` never rewrites prose.
    lines: list[str] = []
    # One entry per line, always. `#[rustfmt::skip]` keeps rustfmt from reflowing the table, so
    # `--write` is idempotent and a diff shows module changes only.
    lines.append("#[rustfmt::skip]")
    lines.append("pub const MODULES: &[ModuleDef] = &[")
    for name, priority, hooks in rows:
        lines.append(
            f'    ModuleDef::new("{name}", {priority_literal(priority)}, {hooks_literal(hooks)}),'
        )
    lines.append("];")
    lines.append("")
    lines.append("/// Module names, sorted, with their index into [`MODULES`].")
    lines.append("///")
    lines.append("/// A sorted array, not a map: a lookup is a scan of 109 pairs, and the order is pinned by")
    lines.append("/// `tests/registry.rs`, so a binary search could replace the scan without changing behaviour.")
    lines.append("#[rustfmt::skip]")
    lines.append("pub const NAME_INDEX: &[(&str, u16)] = &[")
    for name, index in names:
        lines.append(f'    ("{name}", {index}),')
    lines.append("];")
    return "\n".join(lines)


def summary(rows: list[tuple[str, float | int, list[str]]]) -> str:
    counts = {hook: sum(1 for _, _, hooks in rows if hook in hooks) for hook in HOOKS}
    implemented = sum(1 for _, _, hooks in rows if hooks)
    return (
        f"// {len(rows)} modules, {implemented} with hooks: "
        + ", ".join(f"{hook}={count}" for hook, count in counts.items())
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--modules",
        type=pathlib.Path,
        default=pathlib.Path(__file__).resolve().parent.parent / "bot" / "modules",
        help="directory holding the Python modules (default: bot/modules)",
    )
    parser.add_argument(
        "--write",
        action="store_true",
        help="rewrite src/modules/registry.rs in place (a no-op without --write)",
    )
    parser.add_argument(
        "--summary",
        action="store_true",
        help="print a short summary to stderr",
    )
    args = parser.parse_args(argv)

    rows = load(args.modules)
    table = render(rows)
    if args.summary:
        print(summary(rows), file=sys.stderr)

    if not args.write:
        print(table)
        return 0

    target = pathlib.Path(__file__).resolve().parent.parent / "src" / "modules" / "registry.rs"
    original = target.read_text(encoding="utf-8")
    # Replace from the (optional) attribute above the table through the end of the name index:
    # the generator owns the attributes too, so repeating `--write` cannot stack them.
    begin = original.index("pub const MODULES: &[ModuleDef] = &[")
    while begin > 0 and original[:begin].endswith("#[rustfmt::skip]\n"):
        begin -= len("#[rustfmt::skip]\n")
    end = original.index("];", original.index("pub const NAME_INDEX")) + len("];")
    updated = original[:begin] + table + original[end:]
    if updated == original:
        print("registry.rs is already up to date", file=sys.stderr)
        return 0
    target.write_text(updated, encoding="utf-8")
    print(f"updated {target}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
