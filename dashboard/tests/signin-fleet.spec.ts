import { expect, test } from "@playwright/test";
import { adminApi, devstack, newUser, signIn, totp } from "./devstack";

test("sign in with name and password and open a Device", async ({ page }) => {
  const user = await newUser();
  await signIn(page, user);

  await expect(page.getByRole("heading", { name: /Device/ })).toContainText("5 Online · 2 Offline");
  const row = (name: string) => page.getByRole("row").filter({ hasText: name });
  await expect(row("alpha")).toContainText("Online");
  await expect(row("bravo")).toContainText("Offline");
  await expect(page.getByRole("navigation")).toContainText(user.name);

  await page.getByRole("link", { name: "alpha" }).click();
  await expect(page.getByRole("heading", { name: /alpha/ })).toContainText("Online");
  const hardware = page.getByRole("region", { name: "Phần cứng" });
  await expect(hardware).toContainText("FakeOS 1.0");
  await expect(hardware).toContainText("Fake CPU 9000 · 8 nhân");
  await expect(hardware).toContainText("eth0 · 02:00:00:00:00:01 · 10.0.0.11");
  await expect(hardware).toContainText("Display 1 · 1920×1080");
  await expect(hardware).toContainText("FAKE-0001");
  const resources = page.getByRole("region", { name: "Tài nguyên" });
  await expect(resources).toContainText("Màn khoá · an");
  await expect(resources).toContainText("CPU13%"); // FakePlatform reports 12.5 %
  await expect(resources).toContainText("4.0 GB / 16.0 GB");

  // The Control Lock card lives on Màn hình.
  await page.getByRole("link", { name: "Màn hình" }).click();
  const lock = page.getByRole("region", { name: "Control Lock" });
  await expect(lock).toContainText("Không ai giữ");

  // Another Controller (the devstack admin) takes the Control Lock; the page shows the holder.
  await adminApi("/api/v1/devices/alpha/lock", {});
  try {
    await page.reload();
    await expect(lock).toContainText("🔒 admin");
  } finally {
    await adminApi("/api/v1/devices/alpha/lock/release", {});
  }
});

test("a User turns TOTP on with one code, then sign-in asks for it; pages need a session", async ({ page }) => {
  await page.goto("/");
  await expect(page).toHaveURL(/\/signin$/);

  const user = await newUser();
  await signIn(page, { ...user, password: "wrong password" });
  await expect(page.getByRole("alert").filter({ hasText: "Sai" })).toHaveText("Sai tên đăng nhập, mật khẩu hoặc mã TOTP.");
  await expect(page).toHaveURL(/\/signin$/);

  await signIn(page, user);
  await expect(page.getByRole("navigation")).toContainText(user.name);
  await page.goto("/admin/users");
  const mine = page.getByRole("region", { name: "TOTP của bạn" });
  await mine.getByRole("button", { name: "Bật TOTP" }).click();
  await expect(mine.getByRole("img", { name: /Mã QR TOTP/ })).toBeVisible();
  const code = totp((await mine.locator("code").textContent())!, (await devstack()).now);
  await mine.getByLabel("Mã TOTP").fill(code);
  await mine.getByRole("button", { name: "Xác nhận" }).click();
  await expect(mine).toContainText("TOTP đang bật.");

  await page.getByRole("button", { name: "Đăng xuất" }).click();
  await signIn(page, user);
  await expect(page.getByRole("alert").filter({ hasText: "TOTP" })).toHaveText("Tài khoản này đã bật TOTP: nhập mã từ ứng dụng xác thực.");
  await signIn(page, { ...user, code });
  await expect(page.getByRole("navigation")).toContainText(user.name);
});

test("a User adds a Device to their own Address Book by Device ID and Device Password", async ({ page, browser }) => {
  const { devices } = await devstack();
  await signIn(page, await newUser({ empty: true }));
  await expect(page.getByText("Chưa có Device nào.")).toBeVisible();

  const add = page.getByRole("form", { name: "Thêm Device" });
  await add.getByLabel("Device ID").fill(devices.alpha.device_id);
  await add.getByLabel("Device Password").fill("wrongpassword");
  await add.getByRole("button", { name: "Thêm Device" }).click();
  await expect(add.getByRole("alert")).toContainText("unauthorized");
  await expect(page.getByText("Chưa có Device nào.")).toBeVisible();

  await add.getByLabel("Device Password").fill(devices.alpha.password);
  await add.getByLabel("Tên").fill("my-alpha");
  await add.getByRole("button", { name: "Thêm Device" }).click();
  await expect(page.getByRole("row").filter({ hasText: "my-alpha" })).toContainText("Online");
  await page.getByRole("link", { name: "my-alpha" }).click();
  await expect(page.getByRole("heading", { name: /alpha/ })).toContainText("Online");

  // Another User does not see it.
  const other = await browser.newPage();
  await signIn(other, await newUser({ empty: true }));
  await expect(other.getByText("Chưa có Device nào.")).toBeVisible();
  await other.close();
});

test("sign out ends the session", async ({ page }) => {
  await signIn(page, await newUser());
  await page.getByRole("button", { name: "Đăng xuất" }).click();
  await expect(page).toHaveURL(/\/signin$/);
  await page.goto("/");
  await expect(page).toHaveURL(/\/signin$/);
});
