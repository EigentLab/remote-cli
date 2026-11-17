"use server";
// Admin server actions. Each returns a Result for <Form/>.
import QRCode from "qrcode";
import { api, json } from "@/lib/api";
import { run } from "./run";

export type Result = { message?: string; error?: string };

const str = (form: FormData, key: string) => String(form.get(key) ?? "");
const seg = encodeURIComponent;


export async function createUser(_: Result, form: FormData) {
  return run("/admin", async () => {
    await api("/api/v1/users", json("POST", { name: str(form, "name") }));
  });
}

export async function setPassword(_: Result, form: FormData) {
  return run("/admin", async () => {
    await api(`/api/v1/users/${seg(str(form, "user"))}/credentials`, json("POST", { password: str(form, "password") }));
    return "Đã đặt mật khẩu";
  });
}

export type TotpEnrolment = { secret: string; url: string; qr: string };

/** A fresh TOTP secret and its QR code; TOTP turns on only when `turnOnTotp` confirms a code. */
export async function newTotp(): Promise<TotpEnrolment> {
  const r = await api<{ totp_secret: string; otpauth_url: string }>("/api/v1/me/totp", json("POST"));
  return { secret: r.totp_secret, url: r.otpauth_url, qr: await QRCode.toDataURL(r.otpauth_url) };
}

export async function turnOnTotp(_: Result, form: FormData) {
  return run("/admin/users", () => api("/api/v1/me/totp", json("PUT", { secret: str(form, "secret"), code: str(form, "code") })));
}

export async function turnOffTotp(_: Result, form: FormData) {
  return run("/admin/users", () => api("/api/v1/me/totp", json("DELETE", { code: str(form, "code") })));
}

export async function disableUser(_: Result, form: FormData) {
  return run("/admin", () => api(`/api/v1/users/${seg(str(form, "user"))}/disable`, json("POST")));
}

/** Adds a Device to the signed-in User's Address Book; the Server checks the Device Password. */
export async function addToAddressBook(_: Result, form: FormData) {
  const name = str(form, "name").trim();
  return run("/", async () => {
    await api(
      "/api/v1/address-book",
      json("POST", { device_id: str(form, "device_id").trim(), password: str(form, "password"), name: name || undefined }),
      // A wrong Device Password is a 401 too: show it rather than sign the User out.
      { signInOn401: false },
    );
    return "Đã thêm";
  });
}

const entry = (form: FormData) => `/api/v1/address-book/${seg(str(form, "device"))}`;

/** Renames a Device in the signed-in User's Address Book (the name only this User sees). */
export async function renameInAddressBook(_: Result, form: FormData) {
  return run("/", async () => {
    await api(entry(form), json("PATCH", { name: str(form, "name") }));
    return "Đã lưu";
  });
}

/** Removes a Device and its remembered password from the signed-in User's Address Book. */
export async function removeFromAddressBook(_: Result, form: FormData) {
  return run("/", () => api(entry(form), json("DELETE")));
}

/** Deletes one Agent Build (`target` set) or every target of a version. */
export async function deleteBuild(_: Result, form: FormData) {
  const target = str(form, "target");
  const path = `/api/v1/agent-builds/${seg(str(form, "version"))}${target ? `/${seg(target)}` : ""}`;
  return run("/admin/updates", () => api(path, json("DELETE")));
}
