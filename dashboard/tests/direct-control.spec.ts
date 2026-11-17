import { expect, test } from "@playwright/test";
import { adminApi, newUser, signIn } from "./devstack";

// Uses charlie: alpha's Control Lock is asserted on by signin-fleet.spec.ts in parallel.
test("wait for the Control Lock, take it, then drive the Device with mouse and keyboard", async ({ page }) => {
  const user = await newUser();
  // Each input Action the view sends, with the Server's answer.
  const sent: { action: { x?: number; y?: number; display?: number; [k: string]: unknown }; status: number }[] = [];
  page.on("response", (r) => {
    if (r.request().method() === "POST" && r.url().endsWith("/actions")) sent.push({ action: JSON.parse(r.request().postData()!).actions[0], status: r.status() });
  });
  // Another Controller (the devstack admin) holds the lock.
  await adminApi("/api/v1/devices/charlie/lock", {});
  let released = false;
  try {
    await signIn(page, user);
    await page.getByRole("link", { name: "charlie" }).click();
    await page.getByRole("link", { name: "Màn hình" }).click();

    const lock = page.getByRole("region", { name: "Control Lock" });
    await expect(lock).toContainText("🔒 admin");
    await expect(lock).toContainText("hết hạn lúc");

    const live = page.getByRole("region", { name: "Live view" });
    await live.getByRole("button", { name: "Xem trực tiếp" }).click();
    const video = live.getByLabel("Màn hình charlie");
    // Native size: charlie's primary display is 1920x1080.
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.videoWidth), { timeout: 15_000 }).toBe(1920);
    // Without the lock the view is watch-only.
    await expect(live).toContainText("Chỉ xem: cần giữ Control Lock để điều khiển.");

    // Wait like any Controller: no pre-emption; the lock is taken once the holder lets go.
    await lock.getByRole("button", { name: "Chờ Control Lock" }).click();
    await expect(lock).toContainText("Đang chờ");
    await expect(lock).toContainText("🔒 admin");
    await adminApi("/api/v1/devices/charlie/lock/release", {});
    released = true;
    await expect(lock).toContainText(`🔒 ${user.name}`, { timeout: 15_000 });
    await expect(live).not.toContainText("Chỉ xem");

    // Click the middle of the view, then type: the Action carries the streamed display's native pixels (1920x1080),
    // not the Observation grid AI agents see.
    const box = (await video.boundingBox())!;
    await video.click({ position: { x: box.width / 2, y: box.height / 2 } });
    await page.keyboard.press("h");
    await page.keyboard.press("Control+Enter");

    // Each input went through the Action Router and the Server accepted it.
    await expect
      .poll(() =>
        sent.map(({ action: { x, y, ...a }, status }) =>
          // The view's pixel grid isn't the image's, so a point lands within a pixel of the target.
          x === undefined ? { ...a, status } : { ...a, status, near: Math.hypot(x - 960, y! - 540) <= 1.5 },
        ),
      )
      .toEqual([
        { type: "click", display: 0, native: true, near: true, status: 200 },
        // Typed text may be a password: the Dashboard always marks it secret.
        { type: "type", text: "h", secret: true, status: 200 },
        { type: "key", keys: "ctrl+enter", status: 200 },
      ]);

    // charlie has a second display: pick it and the view streams (and clicks on) that one.
    await live.getByLabel("Chọn màn hình").selectOption("1");
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => `${v.videoWidth}x${v.videoHeight}`), { timeout: 15_000 }).toBe("1024x768");
    await video.click({ position: { x: 1, y: 1 } });
    await expect.poll(() => sent.at(-1)?.action.display).toBe(1);

    await lock.getByRole("button", { name: "Trả Control Lock" }).click();
    await expect(lock).toContainText("Không ai giữ");
    await expect(live).toContainText("Chỉ xem");
  } finally {
    if (!released) await adminApi("/api/v1/devices/charlie/lock/release", {});
  }
});
