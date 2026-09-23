// Validates every GraphQL document the core sends against GitHub's published
// schema. Run with `pnpm validate:graphql`. Downloads the schema on first use.
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { buildSchema, parse, validate } from "graphql";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const schemaPath = join(root, "target", "github-schema.graphql");
const docsDir = join(root, "crates", "core", "src", "github", "graphql");

if (!existsSync(schemaPath)) {
  const res = await fetch("https://docs.github.com/public/fpt/schema.docs.graphql");
  if (!res.ok) throw new Error(`schema download failed: ${res.status}`);
  mkdirSync(dirname(schemaPath), { recursive: true });
  writeFileSync(schemaPath, await res.text());
}
const schema = buildSchema(readFileSync(schemaPath, "utf8"), { assumeValidSDL: true });

let failed = false;
for (const file of readdirSync(docsDir).filter((f) => f.endsWith(".graphql")).sort()) {
  const errors = validate(schema, parse(readFileSync(join(docsDir, file), "utf8")));
  if (errors.length) {
    failed = true;
    for (const e of errors) console.error(`${file}: ${e.message}`);
  } else {
    console.log(`ok  ${file}`);
  }
}
process.exit(failed ? 1 : 0);
