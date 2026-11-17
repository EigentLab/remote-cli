import { defineConfig } from "@playwright/test";

// Runs against the in-process system (crates/testkit/src/bin/rcli-devstack.rs): real Server on Postgres in a
// test container + FakePlatform Agents, with a manual clock. Needs Docker (export DOCKER_HOST for Colima).
// The `rcli` binary is built too: specs run it against the Dashboard URL.
// Overridable so several worktrees can run the suite at once.
const API_PORT = Number(process.env.RCLI_TEST_API_PORT ?? 18080);
const WEB_PORT = Number(process.env.RCLI_TEST_WEB_PORT ?? 3100);
// The next ports up: a second Server on the same database whose HTTPS certificate comes from its CA, and a Dashboard
// on it (tests/connect-guide.spec.ts).
const CA_API_PORT = API_PORT + 1;
const CA_WEB_PORT = WEB_PORT + 1;

export default defineConfig({
  testDir: "tests",
  timeout: 30_000,
  use: { baseURL: `http://127.0.0.1:${WEB_PORT}`, trace: "retain-on-failure" },
  webServer: [
    {
      command: `cargo build -q -p rcli-testkit --bin rcli-devstack -p rcli --bin rcli && ../target/debug/rcli-devstack ${API_PORT} .devstack.json ${CA_API_PORT}`,
      url: `http://127.0.0.1:${API_PORT}/api/v1/server-ca`,
      timeout: 600_000,
      reuseExistingServer: false,
      gracefulShutdown: { signal: "SIGTERM", timeout: 10_000 },
      stdout: "pipe",
    },
    {
      command: `pnpm build && pnpm start -p ${WEB_PORT}`,
      url: `http://127.0.0.1:${WEB_PORT}/signin`,
      timeout: 300_000,
      reuseExistingServer: false,
      env: { RCLI_API_URL: `http://127.0.0.1:${API_PORT}` },
    },
    {
      // Starts after the one above has built.
      command: `pnpm start -p ${CA_WEB_PORT}`,
      url: `http://127.0.0.1:${CA_WEB_PORT}/signin`,
      timeout: 60_000,
      reuseExistingServer: false,
      env: { RCLI_API_URL: `http://127.0.0.1:${CA_API_PORT}` },
    },
  ],
});
