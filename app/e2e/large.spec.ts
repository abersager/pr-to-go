import { expect, test } from "@playwright/test";

// A 400-file PR with a 20,000-line new file: everything must stay usable.
// Timings are printed so regressions are easy to spot; the limits are loose
// enough for a slow CI machine.
test.beforeEach(async ({ request }) => {
  await request.post("http://127.0.0.1:1421/__demo/reset");
});

test("large pull request stays responsive", async ({ page, request }) => {
  const { number } = await (await request.post("http://127.0.0.1:1421/__demo/large")).json();
  let t = Date.now();
  const res = await request.post("http://127.0.0.1:1421/api/add_pr", {
    data: { input: `acme/widgets#${number}` },
  });
  const id = await res.json();
  const timings: Record<string, number> = { sync: Date.now() - t };

  t = Date.now();
  await page.goto(`/#/pr/${id}/files`);
  await expect(page.locator(".file-list li")).toHaveCount(402);
  timings.fileList = Date.now() - t;

  t = Date.now();
  await page.locator(".file-list li", { hasText: "src/generated/schema.rs" }).click();
  await expect(page.locator(".diff-scroll")).toContainText("pub fn handler_4242_0(");
  timings.openBigFile = Date.now() - t;
  await expect(page.getByText("too large for syntax highlighting")).toBeVisible();

  t = Date.now();
  await page.locator(".diff-scroll").evaluate((el) => el.scrollTo(0, el.scrollHeight));
  await expect(page.locator(".diff-scroll")).toContainText("handler_4242_19992");
  timings.scrollToEnd = Date.now() - t;

  t = Date.now();
  await page.getByRole("button", { name: "Unified" }).click();
  await expect(page.locator(".diff-scroll")).toContainText("handler_4242_");
  timings.toggleMode = Date.now() - t;

  // The long modified file, with 100 hunks.
  t = Date.now();
  await page.locator(".file-list li", { hasText: "src/generated/routes.rs" }).click();
  await expect(page.locator(".diff-scroll")).toContainText("page_size");
  timings.openHunkyFile = Date.now() - t;

  // Commenting still reacts promptly.
  t = Date.now();
  await page.locator(".diff-scroll .ln.commentable").first().click();
  await expect(page.locator(".composer textarea")).toBeVisible();
  timings.openComposer = Date.now() - t;

  console.log("large PR timings (ms):", JSON.stringify(timings));
  await page.screenshot({ path: "test-results/large.png" });
  expect(timings.fileList).toBeLessThan(10_000);
  expect(timings.openBigFile).toBeLessThan(10_000);
  expect(timings.scrollToEnd).toBeLessThan(5_000);
  expect(timings.openComposer).toBeLessThan(3_000);
});
