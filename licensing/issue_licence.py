#!/usr/bin/env python3
"""Issue a licence for one customer computer. Run on the owner's computer.

  python issue_licence.py --code ABCD-EFGH-JKLM --user "Jane Doe" --days 30

The owner key is read from the key file (or typed at a hidden prompt), never from the command line.
Works for ANY app that uses this licence format: pass --prefix/--app/--public-key-file (or have a licensing.json next to it).
"""
import argparse
import csv
import datetime
import getpass
import os
import sys

import _common

_common.use_modules()
import licensing  # noqa: E402


def main():
    cfg = _common.load_config()
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--prefix", default=cfg.get("prefix"))
    ap.add_argument("--app", default=cfg.get("app_id"), help="the app id baked into the app (licences only work in that app)")
    ap.add_argument("--public-key-file", default=cfg.get("public_key_file"))
    ap.add_argument("--var", default=cfg.get("public_key_var", "OWNER_PUBLIC_KEY"))
    ap.add_argument("--public-key", help="the public key hex, instead of reading it from a file")
    ap.add_argument("--key-file", help="the private owner key file (default <prefix>-owner.key, else a hidden prompt)")
    ap.add_argument("--code", required=True, help="the activation code shown on the customer's computer")
    ap.add_argument("--user", required=True, help="the customer's name")
    ap.add_argument("--days", type=int, required=True)
    ap.add_argument("--features", default="", help="optional comma-separated feature flags")
    ap.add_argument("--log", help="append a line (date, name, code, expiry) to this file; keep it outside the project repo")
    a = ap.parse_args()
    if not (a.prefix and a.app):
        ap.error("--prefix and --app are required (or keep licensing.json next to this script)")

    public = _common.resolve_public_key(a.public_key, a.public_key_file, a.var, cfg)
    if not public:
        ap.error("Can't find the app's public key: put \"public_key\" in licensing.json, or give --public-key or --public-key-file")
    lic = licensing.Licensing(a.app, a.prefix, public, store=None)

    key_file = a.key_file or f"{a.prefix.lower()}-owner.key"
    if not os.path.exists(key_file) and not a.key_file:
        beside = os.path.join(_common.HERE, f"{a.prefix.lower()}-owner.key")   # the key kept next to the tools
        key_file = beside if os.path.exists(beside) else key_file
    if os.path.exists(key_file):
        key_text = open(key_file, encoding="utf-8").read()
    else:
        key_text = getpass.getpass(f"Owner key ({key_file} not found; paste it, input is hidden): ")
    try:
        seed = lic.parse_owner_key(key_text)
        out = lic.issue(seed, a.code, a.user, a.days, [f for f in a.features.split(",") if f.strip()])
    except (licensing.LicenceError, ValueError) as e:
        sys.exit(f"Not issued: {e}")
    if a.log:
        # csv quotes names that contain commas or quotes, so one customer is always one row.
        with open(a.log, "a", encoding="utf-8", newline="") as f:
            csv.writer(f).writerow([datetime.date.today().isoformat(), out["user"], a.code.upper(), out["expires"]])
    print(f"""
Licence for {out['user']}, computer {a.code.upper()}, valid until {out['expires']}.
Send them everything between the lines:
------------------------------------------------------------
{out['licence']}
------------------------------------------------------------
Never send them the owner key.
""")


if __name__ == "__main__":
    main()
