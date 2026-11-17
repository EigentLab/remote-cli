import { expect, test, type Locator, type Page } from "@playwright/test";
import { newUser, signIn } from "./devstack";

test("signed-in pages share a sidebar that marks the current entry", async ({ page }) => {
  await signIn(page, await newUser());
  await expect(page).toHaveTitle("rcli");
  const nav = page.getByRole("navigation");
  await expectWordmark(nav.getByRole("link", { name: "rcli, về danh sách Device" }));
  // Favicons: the SVG mark and an .ico for browsers without SVG icons.
  await expect(page.locator('link[rel="icon"]')).toHaveCount(2);
  for (const icon of await page.locator('link[rel="icon"]').all()) {
    const res = await page.request.get((await icon.getAttribute("href"))!);
    expect(res.headers()["content-type"]).toMatch(/^image\//);
  }
  const device = nav.getByRole("link", { name: "Device", exact: true }).first();
  await expect(device).toHaveAttribute("aria-current", "page");
  await expect(nav.getByRole("link", { name: "Quản lý" })).toHaveCount(1); // every User, no roles (ADR 0009)
  await expect(nav).toContainText("Đăng xuất");

  // The Device entry stays current on a Device page; the sidebar persists across navigation.
  await page.getByRole("link", { name: "alpha" }).click();
  await expect(page.getByRole("heading", { name: /alpha/ })).toBeVisible();
  await expect(device).toHaveAttribute("aria-current", "page");
});

test("a User sees Quản lý marked when on it", async ({ page }) => {
  await signIn(page, await newUser());
  await page.getByRole("link", { name: "Quản lý" }).click();
  await expect(page.getByRole("heading", { name: "Quản lý" })).toBeVisible();
  const nav = page.getByRole("navigation");
  await expect(nav.getByRole("link", { name: "Quản lý" })).toHaveAttribute("aria-current", "page");
  await expect(nav.getByRole("link", { name: "Device", exact: true }).first()).not.toHaveAttribute("aria-current", "page");
});

test("the sign-in page has no sidebar", async ({ page }) => {
  await page.goto("/signin");
  await expect(page.getByRole("button", { name: "Đăng nhập" })).toBeVisible();
  await expect(page.getByRole("navigation")).toHaveCount(0);
  await expect(page).toHaveTitle("rcli");
  await expectWordmark(page);
});

/** The mark and wordmark image, loaded. */
async function expectWordmark(scope: Page | Locator) {
  const img = scope.getByRole("img", { name: "rcli" });
  await expect(img).toBeVisible();
  await expect.poll(() => img.evaluate((i: HTMLImageElement) => i.naturalWidth)).toBeGreaterThan(0);
}
