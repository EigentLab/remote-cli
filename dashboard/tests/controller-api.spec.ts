import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { expect, test } from "@playwright/test";
import { adminApi, devstack } from "./devstack";

// The Controller API through the Dashboard URL (ADR 0006): a Controller given only the Dashboard's address uses the
// real `rcli` binary (built next to rcli-devstack) for commands, the shell stream and `rcli mcp`.
const RCLI = "../target/debug/rcli";

async function rcli(baseURL: string, args: string[], stdin = "") {
  const { admin_token } = await devstack();
  const child = spawn(RCLI, ["--server", baseURL, "--token", admin_token, ...args]);
  let out = "";
  child.stdout.on("data", (d) => (out += d));
  child.stderr.on("data", (d) => (out += d));
  child.stdin.end(stdin);
  const code = await new Promise<number | null>((r) => child.on("close", r));
  return { code, out };
}

test("rcli commands and the shell stream work through the Dashboard URL", async ({ baseURL }) => {
  // `rcli devices` lists the local Address Book (empty here); a name resolves through the User's book on the Server.
  const device = await rcli(baseURL!, ["device", "show", "alpha"]);
  expect(device.code, device.out).toBe(0);
  expect(device.out).toContain("Fake CPU 9000");

  // alpha sits at its lock screen with user "an" logged in: the shell runs as that user, no Control Lock needed.
  const shell = await rcli(baseURL!, ["shell", "alpha"], "echo hi-through-dashboard\nexit 3\n");
  expect(shell.out).toContain("hi-through-dashboard");
  expect(shell.code, shell.out).toBe(3);
});

// juliet: Online, and no other spec takes its Control Lock.
test("rcli mcp holds the Control Lock through the Dashboard URL; it ends 60 s after the holder disconnects", async ({ baseURL }) => {
  const { admin_token, now } = await devstack();
  const mcp = spawn(RCLI, ["--server", baseURL!, "--token", admin_token, "mcp"]);
  const replies = new Map<number, (v: { result?: { structuredContent?: unknown; isError?: boolean } }) => void>();
  createInterface({ input: mcp.stdout }).on("line", (line) => {
    const msg = JSON.parse(line);
    replies.get(msg.id)?.(msg);
  });
  let id = 0;
  const call = (method: string, params: object) =>
    new Promise<{ result?: { structuredContent?: unknown; isError?: boolean } }>((resolve) => {
      replies.set(++id, resolve);
      mcp.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
    });
  const secondsLeft = async () => {
    const lock = await adminApi("/api/v1/devices/juliet/lock");
    return lock.holder === null ? null : Math.floor((Date.parse(lock.expires_at) - now * 1000) / 1000); // `now` is whole seconds
  };

  try {
    await call("initialize", { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "pw", version: "0" } });
    mcp.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");
    const shown = await call("tools/call", { name: "device", arguments: { device: "juliet" } });
    expect(shown.result?.isError ?? false, JSON.stringify(shown)).toBe(false);

    const taken = await call("tools/call", { name: "lock_acquire", arguments: { device: "juliet" } });
    expect(taken.result?.isError ?? false, JSON.stringify(taken)).toBe(false);
    // The watch is open through the Dashboard: only the 10 min idle timer counts.
    expect(await secondsLeft()).toBe(600);

    // The MCP process dies: the Dashboard closes the watch upstream, and the Server gives the holder 60 s.
    mcp.kill();
    await expect.poll(secondsLeft, { timeout: 10_000 }).toBe(60);
  } finally {
    mcp.kill();
    await adminApi("/api/v1/devices/juliet/lock/release", {});
  }
});

// A TLS proxy in front of the Dashboard appends the Controller's IP to X-Forwarded-For; the Dashboard passes it on,
// so the Server (which trusts the Dashboard) rate-limits that Controller, not the Dashboard.
test("the Controller's IP reaches the Server through the Dashboard", async ({ request }) => {
  const { server_key } = await adminApi("/api/v1/server-key");
  const alpha = (await adminApi("/api/v1/address-book")).find((d: { name: string }) => d.name === "alpha");
  const res = await request.get(`/api/v1/devices/${alpha.device_id}`, {
    headers: {
      "x-forwarded-for": "203.0.113.7",
      "x-rcli-server-key": server_key,
      "x-rcli-device": alpha.device_id,
      "x-rcli-device-password": "wrong password",
      "x-rcli-controller": "ip-test",
    },
  });
  expect(res.status()).toBe(401);
});
