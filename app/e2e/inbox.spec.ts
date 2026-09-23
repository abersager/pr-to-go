import { expect, test } from "@playwright/test";

test.beforeEach(async ({ request }) => {
  await request.post("http://127.0.0.1:1421/__demo/reset");
});

test("follow review requests and pack everything for offline", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "⚙" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "+ Review requested from me" }).click();
  await expect(settings.locator(".sub-list")).toContainText("Review requested from me");
  await settings.getByRole("button", { name: "Close" }).click();

  await page.getByRole("button", { name: "Sync all" }).click();
  await expect(page.getByRole("button", { name: "Sync all" })).toBeEnabled({ timeout: 20_000 });
  await expect(page.locator(".readiness")).toContainText("3/3 offline");

  await page.getByRole("button", { name: "⚙" }).click();
  await expect(settings.locator(".sub-list")).toContainText("2 open");
  await expect(settings).toContainText("used.");
  await page.screenshot({ path: "test-results/inbox.png" });
});
