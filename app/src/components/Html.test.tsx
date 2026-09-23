import { expect, test, vi } from "vitest";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
const { sanitize } = await import("./Html");

test("strips scripts and handlers", () => {
  const out = sanitize(`<p onclick="x()">hi<script>alert(1)</script><a href="javascript:alert(1)">x</a></p>`);
  expect(out).not.toContain("script");
  expect(out).not.toContain("onclick");
  expect(out).not.toContain("javascript:");
});

test("keeps local images and replaces remote ones", () => {
  const out = sanitize(
    `<img src="prtg://localhost/asset/abc" alt="a"><img src="https://evil.example/track.png" alt="pixel">`,
  );
  // Outside Tauri, local URLs go through the dev server.
  expect(out).toContain(`src="/prtg/asset/abc"`);
  expect(out).not.toContain("evil.example");
  expect(out).toContain("pixel — not available offline");
});
