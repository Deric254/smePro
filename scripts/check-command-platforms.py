#!/usr/bin/env python3
"""Fail fast if a Tauri command is registered for a platform that doesn't compile it.

Why: the Android job is one of the slowest in CI, and it builds with `desktop` OFF. A command registered in the
`#[cfg(not(desktop))]` handler list must therefore be defined WITHOUT `#[cfg(desktop)]`. Putting a new
command between a doc comment and the `#[cfg(desktop)]` line of the function below it silently moves that
attribute onto the new command (this broke the Android build once). The desktop job can't see it; this can, in seconds.

Checks src-tauri/src/lib.rs: for every command named bare (not `module::name`) in a handler list, the function's own
attribute lines must not say the opposite platform.
"""
import re
import sys

PATH = "src-tauri/src/lib.rs"
src = open(PATH, encoding="utf-8").read()
lines = src.split("\n")

# Each `generate_handler![...]` belongs to the `#[cfg(...)]` line above its builder.
problems = []
for m in re.finditer(r"generate_handler!\[([^\]]*)\]", src):
    names = [n.strip() for n in m.group(1).split(",") if n.strip() and "::" not in n]
    start_line = src[: m.start()].count("\n")
    # nearest preceding `#[cfg(desktop)]` / `#[cfg(not(desktop))]` line
    platform = None
    for i in range(start_line, -1, -1):
        c = re.match(r"\s*#\[cfg\((not\(desktop\)|desktop)\)\]", lines[i])
        if c:
            platform = c.group(1)
            break
    if platform is None:
        problems.append(f"{PATH}:{start_line + 1}: a generate_handler! list has no #[cfg(desktop)] / #[cfg(not(desktop))] above it")
        continue
    forbidden = "desktop" if platform == "not(desktop)" else "not(desktop)"
    for name in names:
        fn_line = next((i for i, l in enumerate(lines) if re.match(rf"\s*(pub\s+)?(async\s+)?fn {re.escape(name)}\b", l)), None)
        if fn_line is None:
            problems.append(f"{PATH}: command `{name}` is registered but no `fn {name}` was found")
            continue
        j = fn_line - 1
        while j >= 0 and lines[j].lstrip().startswith(("#[", "///")):
            if lines[j].strip() == f"#[cfg({forbidden})]":
                problems.append(
                    f"{PATH}:{j + 1}: `{name}` is registered for {'Android/iOS' if platform == 'not(desktop)' else 'desktop'} "
                    f"but is gated with #[cfg({forbidden})], so that build can't find it. "
                    f"Check that no new function was inserted between this attribute and the function it belongs to."
                )
            j -= 1

# And the reverse slip: a desktop-only function that lost its gate (it would then reference desktop-only plugins on mobile).
for i, l in enumerate(lines):
    if re.match(r"\s*async fn rollback_to_manifest\b", l):
        j, gated = i - 1, False
        while j >= 0 and lines[j].lstrip().startswith(("#[", "///")):
            gated = gated or lines[j].strip() == "#[cfg(desktop)]"
            j -= 1
        if not gated:
            problems.append(f"{PATH}:{i + 1}: `rollback_to_manifest` uses the updater plugin and must stay #[cfg(desktop)]")

if problems:
    print("\n".join(problems))
    sys.exit(1)
print("Command registration matches each platform's cfg.")
