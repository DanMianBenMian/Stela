#!/usr/bin/env python3
"""Fetch Stela's pinned host-firmware files into one directory.

`stela create` prefetches the complete host-firmware bundle from
git.kernel.org (its GitLab mirror second). This reads the same pins out of
`recipes.rs` and fetches each file, checking the pinned SHA-256, so the
files can be carried to a computer whose network cannot reach those hosts
and dropped into Stela's cache (`%LOCALAPPDATA%\\stela\\cache\\firmware\\`,
`~/.cache/stela/firmware/`).

Files built from NVIDIA's driver releases (`nvidia/gb202/...`) are skipped:
they are extracted from a slice of the installer by Stela itself.
"""

import argparse
import hashlib
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

RECIPES = "crates/stela-loader/src/host_firmware/recipes.rs"
GITLAB = "https://gitlab.com/api/v4/projects/%s/repository/files/%s/raw?ref=%s"
KORG = "https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/%s?id=%s"
RAWGIT = "https://raw.githubusercontent.com/NVIDIA/open-gpu-kernel-modules/%s/%s"

LINUX_FIRMWARE = ["REALTEK_REVISION", "INTEL_RSA_REVISION", "WIFI_FIRMWARE_REVISION",
                  "MT7925_BLUETOOTH_REVISION", "GSP_REVISION", "XE_REVISION"]


def consts(text):
    found = {}
    for name, value in re.findall(r'const (\w+): &str =\s*\n?\s*"([^"]+)"', text):
        found[name] = value
    return found


def pinned(text):
    """(member, length, sha256, revision) of every artifact the file pins."""
    pairs = []
    patterns = [
        re.compile(r'"([^"\n]+)",\s*(\d+),\s*"([0-9a-f]{64})"'),
        re.compile(r'member:\s*"([^"\n]+)",\s*length:\s*([0-9_]+),\s*sha256:\s*"([0-9a-f]{64})"'),
    ]
    for pattern in patterns:
        for match in pattern.finditer(text):
            before = text[max(0, match.start() - 600):match.start()]
            revision = re.search(r"revision:\s*(\w+)", before)
            pairs.append((match.group(1), int(match.group(2).replace("_", "")),
                          match.group(3), revision.group(1) if revision else None))
    unique = {}
    for member, length, sha, revision in pairs:
        unique.setdefault(sha, (member, length, sha, revision))
    return list(unique.values())


def fetch(url, timeout=300):
    request = urllib.request.Request(url)
    request.add_header("User-Agent", "curl/8")
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.read()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="firmware", help="directory to fill")
    parser.add_argument("--skip-nvidia", action="store_true", default=True)
    options = parser.parse_args()

    text = open(RECIPES, encoding="utf-8").read()
    values = consts(text)
    items = pinned(text)
    os.makedirs(options.out, exist_ok=True)
    print("%d pinned artifacts" % len(items))

    fetched, skipped, failed = 0, 0, []
    for member, length, sha, revision in items:
        path = os.path.join(options.out, sha)
        if os.path.exists(path) and os.path.getsize(path) == length:
            with open(path, "rb") as handle:
                if hashlib.sha256(handle.read()).hexdigest() == sha:
                    fetched += 1
                    continue
        if revision and revision.startswith("OPEN_GPU"):
            urls = [RAWGIT % (values.get(revision, ""), member)]
        elif revision == "WIRELESS_REGDB_REVISION":
            urls = [KORG % (member, values.get(revision, ""))]
        elif revision in LINUX_FIRMWARE or revision is None:
            ref = values.get(revision or "", "main")
            encoded = urllib.parse.quote(member, safe="")
            urls = [
                KORG % (member, ref),
                GITLAB % ("kernel-firmware%2Flinux-firmware", encoded, ref),
            ]
        else:
            # A driver release's member: Stela builds it from the installer.
            print("  skip %-46s (built from NVIDIA's driver)" % member)
            skipped += 1
            continue

        data = None
        for url in urls:
            try:
                data = fetch(url)
            except (urllib.error.HTTPError, urllib.error.URLError, OSError) as error:
                print("  note %-46s %s" % (member, error))
                continue
            if hashlib.sha256(data).hexdigest() == sha and len(data) == length:
                break
            print("  note %-46s hash or length differs" % member)
            data = None
        if data is None:
            failed.append(member)
            print("  FAIL %-46s %d bytes" % (member, length))
            continue
        with open(path, "wb") as handle:
            handle.write(data)
        fetched += 1
        print("  ok   %-46s %8d bytes" % (member, length))

    total = sum(os.path.getsize(os.path.join(options.out, name))
                for name in os.listdir(options.out))
    print("\n%d files, %.1f MiB, %d skipped, %d failed"
          % (fetched, total / 1048576, skipped, len(failed)))
    for member in failed:
        print("  failed: %s" % member)
    return 0


if __name__ == "__main__":
    sys.exit(main())
