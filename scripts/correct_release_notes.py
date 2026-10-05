#!/usr/bin/env python3
"""Owner-authorized, notes-only repair of the existing public RC2/RC3/RC4 metadata.

This is an exception to release asset immutability, not another release build.
It never creates a tag/release, changes publication flags or uploads installers.
The protected signing job requires explicit owner approval for the exception.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

import release

TAGS = ("v0.1.0-rc.2", "v0.1.0-rc.3", "v0.1.0-rc.4")
METADATA = ("SHA256SUMS", "update-manifest.json", "update-manifest.sig")
OLD_STATUS = ("- This candidate is a private draft for acceptance testing. Full native acceptance\n"
              "  on every platform remains pending; it is not a production-stable release.")
NEW_STATUS = ("- Full native acceptance on every platform remains pending; this is a release\n"
              "  candidate, not a production-stable release.")
OLD_CHANNEL = ("- Private drafts are excluded from website downloads and automatic updates.\n"
               "  Automatic updates never downgrade; install this candidate manually for testing.")
NEW_CHANNEL = ("- This release candidate is available through the Beta channel. Automatic\n"
               "  updates never downgrade; select Beta in Preferences → Updates to receive previews.")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def corrected_notes(notes):
    require(isinstance(notes, str) and notes.count(OLD_STATUS) == notes.count(OLD_CHANNEL) == 1,
            "Expected exactly the two original private-draft notes; refuse any other repair")
    return notes.replace(OLD_STATUS, NEW_STATUS).replace(OLD_CHANNEL, NEW_CHANNEL)


def only_notes_changed(before, after):
    expected = dict(before, notes=corrected_notes(before.get("notes")))
    require(after == expected, "Only the two private-draft notes may change; installer/source identity must remain identical")


def gh_json(path):
    return json.loads(subprocess.check_output(["gh", "api", path]))


def snapshot(tag):
    require(tag in TAGS, "This one-time repair is limited to public RC2, RC3 and RC4")
    remote = release.release_for_tag(tag)
    require(remote is not None and remote.get("draft") is False and remote.get("prerelease") is True
            and remote.get("immutable") is False, "Expected an editable, published prerelease")
    expected = set(release.artifact_names(tag[1:]).values()) | set(METADATA)
    assets = remote.get("assets", [])
    require(len(assets) == len(expected) and {a["name"] for a in assets} == expected,
            "Expected exactly five installers and three integrity assets")
    require(all(re.fullmatch(r"sha256:[0-9a-f]{64}", a.get("digest", "")) for a in assets),
            "GitHub must provide SHA256 digests for every existing asset")
    return {
        "release": {key: remote[key] for key in ("id", "tag_name", "draft", "prerelease", "immutable", "published_at", "name", "body")},
        "tag": gh_json(f"repos/{release.REPO}/git/ref/tags/{tag}")["object"],
        "assets": {a["name"]: {key: a[key] for key in ("id", "size", "digest")} for a in assets},
    }


def verify_signature(directory):
    signature = (directory / "update-manifest.sig").read_text().strip()
    require(bool(re.fullmatch(r"[0-9a-f]{128}", signature)), "Invalid signature format")
    public = (release.ROOT / "packaging/update-public-key.hex").read_text().strip()
    require(bool(re.fullmatch(r"[0-9a-f]{64}", public)), "Invalid committed updater trust key")
    with tempfile.TemporaryDirectory() as temporary:
        key, sig = Path(temporary) / "public.der", Path(temporary) / "signature"
        key.write_bytes(bytes.fromhex("302a300506032b6570032100" + public))
        sig.write_bytes(bytes.fromhex(signature))
        subprocess.run(["openssl", "pkeyutl", "-verify", "-pubin", "-keyform", "DER", "-inkey", str(key),
                       "-rawin", "-in", str(directory / "update-manifest.json"), "-sigfile", str(sig)],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def checksums(directory):
    result = {}
    for line in (directory / "SHA256SUMS").read_text().splitlines():
        require(bool(re.fullmatch(r"[0-9a-f]{64}  [A-Za-z0-9.+~-]+", line)), "Invalid checksum record")
        digest, name = line.split("  ")
        require(name not in result, "Duplicate checksum record")
        result[name] = digest
    return result


def validate_preparation(directory):
    baseline = json.loads((directory / "baseline.json").read_text())
    before, after = directory / "before", directory / "after"
    tag = baseline["release"]["tag_name"]
    require(tag in TAGS, "Unexpected repair tag")
    for name in METADATA:
        require("sha256:" + release.sha256(before / name) == baseline["assets"][name]["digest"],
                "Original metadata differs from the recorded GitHub release")
    verify_signature(before)
    original = json.loads((before / "update-manifest.json").read_bytes())
    updated = json.loads((after / "update-manifest.json").read_bytes())
    only_notes_changed(original, updated)
    require(original["tag"] == tag and original["version"] == tag[1:] and original["repository"] == release.REPO
            and original["schema"] == 1 and re.fullmatch(r"[0-9a-f]{40}", original["commit"]), "Invalid original manifest identity")
    require(updated["notes"] == release.notes(tag[1:]), "Prepared correction must match the reviewed changelog exactly")
    expected = release.artifact_names(tag[1:])
    require(len(original["assets"]) == len(expected) and {a["platform"] for a in original["assets"]} == set(expected),
            "Incomplete original manifest")
    old_sums = checksums(before)
    require(set(old_sums) == set(expected.values()) | {"update-manifest.json", "update-manifest.sig"}, "Incomplete original checksums")
    for asset in original["assets"]:
        name = expected[asset["platform"]]
        remote = baseline["assets"][name]
        require(asset["name"] == name and asset["size"] == remote["size"] > 0
                and "sha256:" + asset["sha256"] == remote["digest"] and old_sums[name] == asset["sha256"],
                "Original installer identity disagrees with GitHub/checksums")
    for name in ("update-manifest.json", "update-manifest.sig"):
        require(old_sums[name] == release.sha256(before / name), "Original metadata checksum mismatch")
    return baseline, original


def prepare(tag, directory):
    require(not directory.exists(), "Use a fresh correction directory")
    directory.mkdir(parents=True)
    baseline = snapshot(tag)
    (directory / "baseline.json").write_text(json.dumps(baseline, indent=2) + "\n")
    for name in ("before", "after", "evidence"):
        (directory / name).mkdir()
    with tempfile.TemporaryDirectory() as temporary:
        downloaded = Path(temporary)
        subprocess.run(["gh", "release", "download", tag, "--repo", release.REPO, "--dir", str(downloaded)], check=True)
        require({p.name for p in downloaded.iterdir()} == set(baseline["assets"]), "Downloaded asset set changed")
        for name, asset in baseline["assets"].items():
            path = downloaded / name
            require(path.stat().st_size == asset["size"] and "sha256:" + release.sha256(path) == asset["digest"], "Downloaded asset digest mismatch")
        for name in METADATA:
            shutil.copyfile(downloaded / name, directory / "before" / name)
        original = json.loads((directory / "before/update-manifest.json").read_bytes())
        commit = subprocess.check_output(["git", "rev-parse", f"{tag}^{{commit}}"], text=True).strip()
        require(original["commit"] == commit, "Original manifest source disagrees with the preserved tag")
        subprocess.run(["git", "merge-base", "--is-ancestor", commit, "HEAD"], check=True)
        updated = dict(original, notes=corrected_notes(original["notes"]))
        (directory / "after/update-manifest.json").write_text(json.dumps(updated, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n")
        validate_preparation(directory)
        for name in baseline["assets"]:
            with (directory / "evidence" / f"{name}.original-attestation.json").open("w") as output:
                subprocess.run(["gh", "attestation", "verify", str(downloaded / name), "--repo", release.REPO,
                                "--signer-workflow", f"{release.REPO}/.github/workflows/release.yml", "--source-digest", commit,
                                "--signer-digest", commit, "--source-ref", f"refs/tags/{tag}", "--deny-self-hosted-runners", "--format", "json"],
                               check=True, stdout=output)
    require(snapshot(tag) == baseline, "Release changed during preparation; refuse stale correction")
    print(f"Prepared unsigned {tag} notes correction. Five installers, tag and original source SHA remain identical.")


def seal(directory):
    baseline, original = validate_preparation(directory)
    require(snapshot(baseline["release"]["tag_name"]) == baseline, "Release changed before signing")
    private = os.environ.get("UPDATE_SIGNING_KEY", "")
    require(bool(private), "Missing protected UPDATE_SIGNING_KEY")
    after = directory / "after"
    require({p.name for p in after.iterdir()} == {"update-manifest.json"}, "Expected only the reviewed unsigned manifest before signing")
    with tempfile.TemporaryDirectory() as temporary:
        key, sig = Path(temporary) / "key.pem", Path(temporary) / "signature"
        key.write_text(private)
        key.chmod(0o600)
        public = subprocess.check_output(["openssl", "pkey", "-in", str(key), "-pubout", "-outform", "DER"], stderr=subprocess.DEVNULL)
        expected = bytes.fromhex("302a300506032b6570032100" + (release.ROOT / "packaging/update-public-key.hex").read_text().strip())
        require(public == expected, "Signing key does not match the embedded updater trust anchor")
        subprocess.run(["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(key), "-in", str(after / "update-manifest.json"), "-out", str(sig)], check=True, stderr=subprocess.DEVNULL)
        (after / "update-manifest.sig").write_text(sig.read_bytes().hex() + "\n")
    sums = {a["name"]: a["sha256"] for a in original["assets"]}
    sums.update({name: release.sha256(after / name) for name in ("update-manifest.json", "update-manifest.sig")})
    (after / "SHA256SUMS").write_text("".join(f"{sums[name]}  {name}\n" for name in sorted(sums)))
    verify_signature(after)
    print("Signed the corrected notes only; generated checksums with unchanged installer hashes.")


def verify_published(directory, baseline):
    current = snapshot(baseline["release"]["tag_name"])
    require(current["release"] == baseline["release"] and current["tag"] == baseline["tag"], "Release or tag identity changed")
    for name, old in baseline["assets"].items():
        if name in METADATA:
            path = directory / "after" / name
            require(current["assets"][name]["size"] == path.stat().st_size
                    and current["assets"][name]["digest"] == "sha256:" + release.sha256(path), "Published correction digest mismatch")
        else:
            require(current["assets"][name] == old, "Installer asset identity changed")
    with tempfile.TemporaryDirectory() as temporary:
        command = ["gh", "release", "download", baseline["release"]["tag_name"], "--repo", release.REPO, "--dir", temporary]
        for name in METADATA:
            command.extend(["--pattern", name])
        subprocess.run(command, check=True)
        for name in METADATA:
            require((Path(temporary) / name).read_bytes() == (directory / "after" / name).read_bytes(), "Published metadata bytes differ")
        verify_signature(Path(temporary))
    (directory / "evidence/published.json").write_text(json.dumps(current, indent=2) + "\n")


def rollback(directory, baseline):
    tag = baseline["release"]["tag_name"]
    current = release.release_for_tag(tag)
    require(current is not None and {key: current[key] for key in baseline["release"]} == baseline["release"]
            and gh_json(f"repos/{release.REPO}/git/ref/tags/{tag}")["object"] == baseline["tag"],
            "Release changed independently; refuse to overwrite another operator's work during recovery")
    present = {a["name"]: a for a in current["assets"]}
    for name, old in baseline["assets"].items():
        if name in METADATA:
            permitted = {old["digest"], "sha256:" + release.sha256(directory / "after" / name)}
            require(name not in present or present[name]["digest"] in permitted,
                    "Metadata changed independently; refuse to overwrite another operator's correction")
        else:
            require(name in present and {key: present[name][key] for key in old} == old,
                    "Installer changed independently; stop recovery for owner review")
    subprocess.run(["gh", "release", "upload", tag, "--repo", release.REPO, "--clobber", *[str(directory / "before" / name) for name in METADATA]], check=True)


def publish(directory):
    baseline, original = validate_preparation(directory)
    after = directory / "after"
    require({p.name for p in after.iterdir()} == set(METADATA), "Only three metadata assets may be uploaded")
    verify_signature(after)
    expected_sums = {a["name"]: a["sha256"] for a in original["assets"]}
    expected_sums.update({name: release.sha256(after / name) for name in ("update-manifest.json", "update-manifest.sig")})
    require(checksums(after) == expected_sums, "Corrected checksums must preserve every installer hash")
    tag = baseline["release"]["tag_name"]
    require(snapshot(tag) == baseline, "Release changed before upload; refuse stale correction")
    try:
        subprocess.run(["gh", "release", "upload", tag, "--repo", release.REPO, "--clobber", *[str(after / name) for name in METADATA]], check=True)
        verify_published(directory, baseline)
    except Exception:
        # Restore the original signed set if replacement or verification fails.
        # Download caches may temporarily fail signature checks during replacement.
        rollback(directory, baseline)
        raise
    print(f"Corrected {tag} signed notes. Original installer IDs/hashes, tag and publication flags verified unchanged.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    preparation = commands.add_parser("prepare")
    preparation.add_argument("tag", choices=TAGS)
    preparation.add_argument("directory", type=Path)
    for operation in ("seal", "publish"):
        commands.add_parser(operation).add_argument("directory", type=Path)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.tag, args.directory)
    elif args.command == "seal":
        seal(args.directory)
    else:
        publish(args.directory)


if __name__ == "__main__":
    main()
