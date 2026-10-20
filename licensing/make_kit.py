#!/usr/bin/env python3
"""Make a folder that issues licences on ANY computer, without the project.

  python licensing/make_kit.py [--out smepro-licence-kit]

Copy the folder to the other computer (USB stick, cloud drive) together with your private owner key
(smp-owner.key, put in the same folder or point to it in issue-licence.bat). Python 3 is the only
requirement. The folder holds no secret: the public key it contains can only check licences.
"""
import argparse
import os
import shutil
import sys

import _common

FILES = ["licensing.py", "ed25519.py", "_common.py", "issue_licence.py", "verify_licence.py"]


def main():
    cfg = _common.load_config()
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", default="smepro-licence-kit")
    a = ap.parse_args()
    public = _common.resolve_public_key(None, cfg.get("public_key_file"), cfg.get("public_key_var", "OWNER_PUBLIC_KEY"), cfg)
    if not _common.valid_public_key(public):
        sys.exit("No valid owner public key found (licensing.json \"public_key\" or the app's key file). Run make_owner_key.py first.")
    os.makedirs(a.out, exist_ok=True)
    for name in FILES:
        shutil.copy2(os.path.join(_common.HERE, name), os.path.join(a.out, name))
    _common.save_config({"app_id": cfg["app_id"], "prefix": cfg["prefix"], "public_key": public}, os.path.join(a.out, _common.CONFIG_NAME))
    bat = os.path.join(_common.HERE, "..", "smeprolc", "issue-licence.bat")
    if os.path.exists(bat):
        shutil.copy2(bat, os.path.join(a.out, "issue-licence.bat"))
    print(f"Made {os.path.abspath(a.out)}\nCopy it, with smp-owner.key, to the other computer and run issue-licence.bat (or issue_licence.py).")


if __name__ == "__main__":
    main()
