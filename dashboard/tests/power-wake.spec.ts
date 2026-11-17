import { expect, test } from "@playwright/test";
import { adminApi, newUser, signIn } from "./devstack";

// Uses golf (Online) and hotel/bravo (Offline): other specs don't touch them.
test("power buttons follow the Control Lock like the CLI's", async ({ page }) => {
  const user = await newUser();
  await adminApi("/api/v1/devices/golf/lock", {});
  let released = false;
  try {
    await signIn(page, user);
    await page.getByRole("link", { name: "golf" }).click();
    const power = page.getByRole("region", { name: "Nguồn" });
    await expect(power.getByRole("button", { name: "Wake" })).toBeDisabled();

    // Someone else holds the Control Lock: only Lock screen is allowed.
    await expect(power).toContainText("Control Lock đang do admin giữ");
    for (const name of ["Restart", "Shut down", "Sleep"]) await expect(power.getByRole("button", { name })).toBeDisabled();
    await power.getByRole("button", { name: "Lock screen" }).click();
    await expect(power).toContainText("Đã gửi Lock screen");

    await adminApi("/api/v1/devices/golf/lock/release", {});
    released = true;
    await expect(power.getByRole("button", { name: "Restart" })).toBeEnabled({ timeout: 15_000 });
    // Destructive actions ask first; dismissing sends nothing.
    page.once("dialog", (d) => d.dismiss());
    await power.getByRole("button", { name: "Shut down" }).click();
    page.once("dialog", (d) => d.accept());
    await power.getByRole("button", { name: "Restart" }).click();
    await expect(power).toContainText("Đã gửi Restart");
  } finally {
    if (!released) await adminApi("/api/v1/devices/golf/lock/release", {});
  }
});

test("Wake shows why it can't", async ({ page }) => {
  await signIn(page, await newUser());

  // bravo has not been Online since this User added it: their Address Book has no MAC and subnet for it.
  await page.getByRole("link", { name: "bravo" }).click();
  const power = page.getByRole("region", { name: "Nguồn" });
  await expect(power.getByRole("button", { name: "Restart" })).toHaveCount(0);
  await power.getByRole("button", { name: "Wake" }).click();
  await expect(power).toContainText("it has not been Online since it was added to this Address Book");
});

test("an Offline Device shows Wake instead of Live view and Terminal", async ({ page }) => {
  await signIn(page, await newUser());

  // hotel never comes back; alpha is in its LAN and relays the packet.
  await expect(page.getByRole("link", { name: "hotel" })).toBeVisible(); // signed in
  await expect(page.getByRole("row").filter({ hasText: "hotel" })).toContainText("Offline");
  await page.getByRole("link", { name: "hotel" }).click();
  await expect(page.getByRole("heading", { name: /hotel/ })).toContainText("Offline");

  const placeholder = page.getByRole("region", { name: "Không xem được" });
  await page.getByRole("link", { name: "Màn hình" }).click();
  await expect(placeholder).toContainText("Device đang Offline");
  await expect(page.getByRole("region", { name: "Live view" })).toHaveCount(0);
  await expect(page.getByRole("region", { name: "Control Lock" })).toHaveCount(0);
  await page.getByRole("link", { name: "Terminal" }).click();
  await expect(placeholder).toContainText("Device đang Offline");
  await expect(page.getByRole("region", { name: "Terminal" })).toHaveCount(0);

  // The placeholder's Wake is sent, then it waits for Online.
  await page.getByRole("link", { name: "Màn hình" }).click();
  await placeholder.getByRole("button", { name: "Wake" }).click();
  await expect(placeholder).toContainText("Đang chờ Device Online…");
  // The header's Wake names the relay.
  const power = page.getByRole("region", { name: "Nguồn" });
  await power.getByRole("button", { name: "Wake" }).click();
  await expect(power).toContainText("Đã gửi Wake qua alpha (02:00:00:00:00:05)");
});
