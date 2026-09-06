// Unit tests for the cheat database builder (issue #51). Run with
// `node --test test/*.test.mjs` or `npm test`.
import assert from "node:assert/strict";
import { test } from "node:test";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { buildCheatsJson, databaseCrcs, databaseName, readCheatDir } from "../scripts/build-cheats.mjs";

const SMB =
  "# Super Mario Bros. (World)\n" +
  "# nes-emu cheat database entry: CODE<TAB>1|0<TAB>description\n" +
  "# crc32: 8E2BD25C\n" +
  "# crc32: d26efd78\n" +
  "\n" +
  "SXIOPO\t0\tInfinite lives\n";

test("databaseCrcs reads every header line, normalised to upper case", () => {
  assert.deepEqual(databaseCrcs(SMB), ["8E2BD25C", "D26EFD78"]);
  assert.deepEqual(databaseCrcs("# crc32: abc\n"), ["00000ABC"]);
  assert.deepEqual(databaseCrcs("# crc32: nothex\n#crc32:\nSXIOPO\t1\tx\n"), []);
  assert.deepEqual(databaseCrcs("#crc32:F6035030\r\n"), ["F6035030"]);
});

test("databaseName takes the first comment line", () => {
  assert.equal(databaseName(SMB, "fallback"), "Super Mario Bros. (World)");
  assert.equal(databaseName("# crc32: 1\nSXIOPO\n", "fallback"), "fallback");
  assert.equal(databaseName("SXIOPO\t1\tx\n", "fallback"), "fallback");
});

test("buildCheatsJson maps every CRC to the file body", () => {
  const json = buildCheatsJson([
    { name: "Zelda.cht", text: "# Zelda\n# crc32: 3FE272FB\nAAAA:01\t0\tx\n" },
    { name: "Super Mario Bros.cht", text: SMB },
  ]);
  assert.deepEqual(Object.keys(json).sort(), ["3FE272FB", "8E2BD25C", "D26EFD78"]);
  assert.equal(json["8E2BD25C"].name, "Super Mario Bros. (World)");
  assert.equal(json["8E2BD25C"].text, SMB);
  assert.equal(json["D26EFD78"], json["8E2BD25C"]);
  assert.equal(json["3FE272FB"].name, "Zelda");
});

test("first file in name order wins a duplicate CRC", () => {
  const json = buildCheatsJson([
    { name: "b.cht", text: "# B\n# crc32: 00000001\n" },
    { name: "a.cht", text: "# A\n# crc32: 00000001\n" },
  ]);
  assert.equal(json["00000001"].name, "A");
});

test("bundled cheat files all carry a CRC and are disabled by default", () => {
  const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "cheats");
  const files = readCheatDir(dir);
  assert.ok(files.length >= 9, `found ${files.length} .cht files`);
  const json = buildCheatsJson(files);
  for (const f of files) {
    assert.ok(databaseCrcs(f.text).length > 0, `${f.name} has a crc32 line`);
    for (const line of f.text.split("\n")) {
      if (!line.trim() || line.startsWith("#")) continue;
      const fields = line.split("\t");
      assert.equal(fields[1], "0", `${f.name}: ${line} is disabled`);
    }
  }
  assert.equal(json["8E2BD25C"].name, "Super Mario Bros. (World)");
  assert.match(json["8E2BD25C"].text, /SXIOPO/);
});
