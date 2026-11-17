// Fixtures for Dashboard tests: the devstack's admin API access and signing in a fresh user.
import { createHmac } from "node:crypto";
import { readFile } from "node:fs/promises";
import { expect, type Page } from "@playwright/test";

type Info = {
  api: string;
  admin_token: string;
  now: number;
  /** An Agent Build to upload. */
  update: { version: string; target: string; build: string };
  /** Each devstack Device's ID and Device Password, by name; an Offline one may carry its last MAC and subnet. */
  devices: Record<string, { device_id: string; password: string; mac?: string; subnet?: string }>;
};

/** Waits for rcli-devstack to finish seeding the fleet (it writes .devstack.json when ready). */
export async function devstack(): Promise<Info> {
  for (let i = 0; i < 600; i++) {
    try {
      return JSON.parse(await readFile(".devstack.json", "utf8"));
    } catch {
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  throw new Error(".devstack.json not written within 60s");
}

/** Controller API call as the devstack admin. */
export async function adminApi(path: string, body?: unknown) {
  const { api, admin_token } = await devstack();
  const res = await fetch(api + path, {
    method: body === undefined ? "GET" : "POST",
    headers: { authorization: `Bearer ${admin_token}`, "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  expect(res.ok, `${path}: ${res.status}`).toBeTruthy();
  return res.json();
}

/** RFC 6238 code (SHA-1, 30 s, 6 digits) for a base32 secret at `unixSecs` (the devstack's frozen clock). */
export function totp(secretBase32: string, unixSecs: number): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = "";
  for (const c of secretBase32) bits += alphabet.indexOf(c).toString(2).padStart(5, "0");
  const key = Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2)));
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(unixSecs / 30)));
  const mac = createHmac("sha1", key).update(counter).digest();
  const o = mac[19] & 0x0f;
  return String((mac.readUInt32BE(o) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}

let users = 0;

/** Creates a fresh user with a password (TOTP off, as for every new User), returning what they type to sign in.
 * Unless `empty`, the whole devstack fleet is in their Address Book under its names. */
export async function newUser({ empty = false } = {}) {
  const name = `user-${process.pid}-${++users}`;
  const password = "correct horse battery";
  await adminApi("/api/v1/users", { name });
  await adminApi(`/api/v1/users/${name}/credentials`, { password });
  if (!empty) {
    const { api, devices } = await devstack();
    const res = await fetch(`${api}/api/v1/sessions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name, password }),
    });
    expect(res.ok, `sign in ${name}: ${res.status}`).toBeTruthy();
    const { token } = await res.json();
    for (const [device, entry] of Object.entries(devices)) {
      const added = await fetch(`${api}/api/v1/address-book`, {
        method: "POST",
        headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
        body: JSON.stringify({ ...entry, name: device }),
      });
      expect(added.ok, `add ${device}: ${added.status}`).toBeTruthy();
    }
  }
  return { name, password };
}

export async function signIn(page: Page, user: { name: string; password: string; code?: string }) {
  await page.goto("/signin");
  await page.getByLabel("Tên đăng nhập").fill(user.name);
  await page.getByLabel("Mật khẩu").fill(user.password);
  if (user.code) await page.getByLabel("Mã TOTP").fill(user.code);
  await page.getByRole("button", { name: "Đăng nhập" }).click();
}
