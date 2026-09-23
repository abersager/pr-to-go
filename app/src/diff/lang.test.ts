import { expect, test } from "vitest";
import { langForPath } from "./lang";

const known = new Set(["rust", "typescript", "ts", "tsx", "yaml", "docker", "python", "make", "json"]);
const has = (l: string) => known.has(l);

test("maps paths to languages", () => {
  expect(langForPath("src/lib.rs", has)).toBe("rust");
  expect(langForPath("a/b.ts", has)).toBe("ts");
  expect(langForPath("App.tsx", has)).toBe("tsx");
  expect(langForPath(".github/ci.yml", has)).toBe("yaml");
  expect(langForPath("Dockerfile", has)).toBe("docker");
  expect(langForPath("Makefile", has)).toBe("make");
  expect(langForPath("notes.unknownext", has)).toBeNull();
  expect(langForPath("LICENSE", has)).toBeNull();
});
