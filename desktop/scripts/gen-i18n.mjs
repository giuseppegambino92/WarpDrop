// Converts the canonical cross-platform strings (../i18n/strings.json) plus the
// desktop-only additions (desktop-strings.json) into src/i18n/{en,it}.json.
// Run with `npm run i18n` whenever either source changes; the output is
// committed so builds do not depend on the repo layout.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const canonical = JSON.parse(readFileSync(join(here, "../../i18n/strings.json"), "utf8"));
const extra = JSON.parse(readFileSync(join(here, "desktop-strings.json"), "utf8"));

const langs = ["en", "it"];
const out = Object.fromEntries(langs.map((l) => [l, {}]));
for (const source of [canonical, extra]) {
  for (const [key, value] of Object.entries(source)) {
    if (key.startsWith("_")) continue;
    for (const l of langs) {
      if (typeof value[l] !== "string") throw new Error(`missing "${l}" for key "${key}"`);
      if (key in out[l] && source === extra) throw new Error(`desktop key "${key}" shadows a canonical key`);
      out[l][key] = value[l];
    }
  }
}
for (const l of langs) {
  writeFileSync(join(here, `../src/i18n/${l}.json`), JSON.stringify(out[l], null, 2) + "\n");
}
console.log(`wrote ${Object.keys(out.en).length} keys for ${langs.join(", ")}`);
