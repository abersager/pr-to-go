import { expect, test, vi } from "vitest";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
const { renderMarkdown, suggestionBlock } = await import("./markdown");

test("renders Markdown safely", () => {
  const html = renderMarkdown("**bold** <script>x</script> [link](https://example.com)");
  expect(html).toContain("<strong>bold</strong>");
  expect(html).not.toContain("<script>");
  expect(html).toContain('href="https://example.com"');
});

test("previews suggestions against the original lines", () => {
  const html = renderMarkdown("Try this:\n\n" + suggestionBlock(["let y = 2;"]), ["let x = 1;"]);
  expect(html).toContain("Suggested change");
  expect(html).toContain('<div class="del">let x = 1;</div>');
  expect(html).toContain('<div class="add">let y = 2;</div>');
});
