#!/usr/bin/env python3
"""Generates the Apple string catalogs from the canonical i18n/strings.json.

    python3 apple/scripts/generate-strings.py

Writes:
  apple/Localization/Localizable.xcstrings  (UI strings, every key of i18n/strings.json
                                             plus apple/Localization/apple-extra-strings.json)
  apple/Localization/InfoPlist.xcstrings    (permission prompts / display name)

Placeholders stay in the shared `{0}`, `{1}` form; the apps substitute them at
runtime (see Shared/Core/Localizer.swift), so one catalog works for every
language without printf-style specifiers.
"""
import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
APPLE = ROOT / "apple"
LANGS = ["en", "it"]


def load(path):
    data = json.loads(path.read_text(encoding="utf-8"))
    return {k: v for k, v in data.items() if not k.startswith("_")}


def unit(value):
    return {"stringUnit": {"state": "translated", "value": value}}


def catalog(entries, comment=None):
    strings = {}
    for key in sorted(entries):
        values = entries[key]
        item = {"extractionState": "manual", "localizations": {lang: unit(values[lang]) for lang in LANGS if lang in values}}
        if comment and key in comment:
            item["comment"] = comment[key]
        strings[key] = item
    return {"sourceLanguage": "en", "strings": strings, "version": "1.0"}


def write(path, obj):
    path.write_text(json.dumps(obj, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {path.relative_to(ROOT)} ({len(obj['strings'])} keys)")


def main():
    canonical = load(ROOT / "i18n" / "strings.json")
    extra = load(APPLE / "Localization" / "apple-extra-strings.json")
    clash = set(canonical) & set(extra)
    if clash:
        raise SystemExit(f"apple-extra-strings.json redefines canonical keys: {sorted(clash)}")
    for key, values in {**canonical, **extra}.items():
        missing = [lang for lang in LANGS if lang not in values]
        if missing:
            raise SystemExit(f"{key}: missing {missing}")
    write(APPLE / "Localization" / "Localizable.xcstrings", catalog({**canonical, **extra}))

    info = {
        "CFBundleDisplayName": canonical["app_name"],
        "CFBundleName": canonical["app_name"],
        "NSLocalNetworkUsageDescription": canonical["permission_local_network"],
        "NSCameraUsageDescription": canonical["permission_camera"],
    }
    write(APPLE / "Localization" / "InfoPlist.xcstrings", catalog(info))


if __name__ == "__main__":
    main()
