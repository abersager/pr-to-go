import { expect, test } from "@playwright/test";

test.beforeEach(async ({ request }) => {
  await request.post("http://127.0.0.1:1421/__demo/reset");
});

test("browse every open PR you can reach and take one offline", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("tab", { name: "Browse" }).click();
  const list = page.locator(".browse li");
  // The three demo PRs in acme/widgets (already offline) and bob's, in a
  // repository he shared.
  await expect(list).toHaveCount(4);
  await expect(page.locator(".browse-scope")).toContainText(
    "Open pull requests in your repositories, acme and 1 shared repository.",
  );
  const bob = list.filter({ hasText: "Colour the zsh prompt" });
  await expect(bob.locator(".sync.none")).toBeVisible();
  await expect(list.filter({ hasText: "Dark mode" }).locator(".sync.ok")).toBeVisible();

  // Picking it syncs it and opens it.
  await bob.click();
  await expect(page.getByRole("heading", { name: /Colour the zsh prompt/ })).toBeVisible();
  await expect(bob.locator(".sync.ok")).toBeVisible();
  await page.screenshot({ path: "test-results/browse.png" });

  // A filter narrows the list.
  await page.getByLabel("Filter pull requests").fill("dark");
  await page.getByRole("button", { name: "Search" }).click();
  await expect(list).toHaveCount(1);
  await expect(list).toContainText("Dark mode for the settings page");

  // The picked PR is in the inbox now.
  await page.getByRole("tab", { name: "Inbox" }).click();
  await expect(page.locator(".inbox li", { hasText: "Colour the zsh prompt" })).toBeVisible();
});

test("browsing offline explains why the list is empty", async ({ page, request }) => {
  await page.goto("/");
  await request.post("http://127.0.0.1:1421/__demo/offline");
  await page.getByRole("tab", { name: "Browse" }).click();
  await expect(page.getByText("Browsing needs a connection. Your inbox works offline.")).toBeVisible();
});
