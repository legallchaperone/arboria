"""List the crates in Cargo.lock files that were published less than two weeks before a
date (the dependency rule in Cargo.toml): `python scripts/check_crate_dates.py
[--before 2026-10-22] Cargo.lock app/src-tauri/Cargo.lock`. Reads publish times from the
crates.io sparse index (`pubtime`). For each crate that is too new it prints the newest
older version with the same major (or 0.minor) so `cargo update -p NAME --precise V` can
pin it. Exit 1 when something is too new."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import sys
import tomllib
import urllib.request
from pathlib import Path

INDEX = "https://index.crates.io"


def index_path(name: str) -> str:
    n = name.lower()
    if len(n) <= 2:
        return f"{len(n)}/{n}"
    if len(n) == 3:
        return f"3/{n[0]}/{n}"
    return f"{n[:2]}/{n[2:4]}/{n}"


def entries(name: str, cache: dict[str, list[dict]]) -> list[dict]:
    if name not in cache:
        req = urllib.request.Request(f"{INDEX}/{index_path(name)}",
                                     headers={"User-Agent": "arboria-dev-dates"})
        with urllib.request.urlopen(req, timeout=30) as r:
            cache[name] = [json.loads(line) for line in r.read().decode().splitlines() if line]
    return cache[name]


def compat(a: str, b: str) -> bool:
    pa, pb = a.split("."), b.split(".")
    if pa[0] != "0":
        return pa[0] == pb[0]
    return pa[:2] == pb[:2]


def key(v: str) -> tuple:
    core = v.split("-")[0].split("+")[0]
    return tuple(int(x) for x in core.split("."))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("locks", nargs="+", type=Path)
    ap.add_argument("--before", help="YYYY-MM-DD (default: 14 days ago)")
    args = ap.parse_args()
    cutoff = (dt.date.fromisoformat(args.before) if args.before
              else dt.date.today() - dt.timedelta(days=14)).isoformat()
    cache: dict[str, list[dict]] = {}
    bad = 0
    for lock in args.locks:
        pkgs = tomllib.loads(lock.read_text())["package"]
        for p in pkgs:
            if not p.get("source", "").startswith("registry+"):
                continue
            es = entries(p["name"], cache)
            e = next((e for e in es if e["vers"] == p["version"]), None)
            when = (e or {}).get("pubtime") or ""
            if when and when[:10] < cutoff:
                continue
            older = [x["vers"] for x in es
                     if (x.get("pubtime") or "9")[:10] < cutoff and not x.get("yanked")
                     and "-" not in x["vers"].split("+")[0]
                     and compat(x["vers"], p["version"])]
            older.sort(key=key)
            print(f"{lock}: {p['name']} {p['version']} published {when or '?'}"
                  f" -> {older[-1] if older else 'no older compatible version'}")
            bad += 1
    print(f"{bad} too new (cutoff {cutoff})", file=sys.stderr)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
