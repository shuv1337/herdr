import { afterEach, beforeEach, expect, mock, spyOn, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";

const requests: unknown[] = [];
const activeDisposers: Array<() => void> = [];
const requestWaiters: Array<() => void> = [];
const stateWaiters: Array<() => void> = [];
let importCounter = 0;
let holdConnections = false;
let failConnections = false;
const connections: Array<() => void> = [];
const realNow = Date.now;
const originalArgv = [...process.argv];
let configDir: string;
let clockOffset = 0;
let clock: ReturnType<typeof spyOn>;

mock.module("node:net", () => ({
  default: {
    createConnection(_path: string, onConnect: () => void) {
      const handlers = new Map<string, () => void>();
      const client = {
        destroyed: false,
        write(input: string) {
          if (client.destroyed) return;
          const request = JSON.parse(input.trim());
          requests.push(request);
          if (isRecord(request) && isRecord(request.params) && request.params.state !== undefined) {
            stateWaiters.shift()?.();
          }
          requestWaiters.shift()?.();
          queueMicrotask(() => client.emit("data"));
        },
        setTimeout() {},
        on(event: string, handler: () => void) {
          handlers.set(event, handler);
        },
        destroy() {
          client.destroyed = true;
        },
        emit(event: string) {
          handlers.get(event)?.();
        },
      };
      if (holdConnections) connections.push(onConnect);
      else if (failConnections) queueMicrotask(() => client.emit("error"));
      else queueMicrotask(onConnect);
      return client;
    },
  },
}));

beforeEach(async () => {
  await mkdir(".local", { recursive: true });
  configDir = await mkdtemp(".local/herdr-tui-config-");
  clockOffset = 0;
  clock = spyOn(Date, "now").mockImplementation(() => realNow() + clockOffset);
  process.argv = [...originalArgv];
  requests.length = 0;
  requestWaiters.length = 0;
  stateWaiters.length = 0;
  holdConnections = false;
  failConnections = false;
  connections.length = 0;
  process.env.HERDR_ENV = "1";
  process.env.HERDR_SOCKET_PATH = "test.sock";
  process.env.HERDR_PANE_ID = "test:p1";
  process.env.OPENCODE_CONFIG_DIR = configDir;
  delete process.env.OPENCODE_CLI_CONFIG_CONTENT;
  delete process.env.HERDR_OPENCODE_TRACE;
});

afterEach(async () => {
  for (const dispose of activeDisposers.splice(0)) {
    dispose();
  }
  clock.mockRestore();
  process.argv = [...originalArgv];
  delete process.env.HERDR_OPENCODE_TRACE;
  delete process.env.OPENCODE_CLI_CONFIG_CONTENT;
  await rm(configDir, { recursive: true, force: true });
});

async function loadPlugin() {
  importCounter += 1;
  const module = await import(`./herdr-tui-session.js?test=${importCounter}`);
  return module.default;
}

function fakeApi() {
  const sessions = new Map<string, { id: string; parentID?: string }>();
  let current: { name: string; params?: { sessionID: string } } = { name: "home" };
  let dispose: (() => void) | undefined;
  activeDisposers.push(() => dispose?.());

  return {
    api: {
      route: {
        get current() {
          return current;
        },
      },
      state: {
        session: {
          get(sessionID: string) {
            return sessions.get(sessionID);
          },
        },
      },
      lifecycle: {
        onDispose(handler: () => void) {
          dispose = handler;
          return () => {};
        },
      },
    },
    addSession(session: { id: string; parentID?: string }) {
      sessions.set(session.id, session);
    },
    select(sessionID: string) {
      current = { name: "session", params: { sessionID } };
    },
    dispose() {
      dispose?.();
    },
  };
}

function waitForNextRequest(): Promise<void> {
  return new Promise((resolve) => requestWaiters.push(resolve));
}

function waitForStateReport(): Promise<void> {
  return new Promise((resolve) => stateWaiters.push(resolve));
}

test("reports a root session when only the local route changes", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "session-a" });
  await plugin.tui(tui.api);

  const dispatched = waitForNextRequest();
  tui.select("session-a");
  await dispatched;

  expect(requests).toHaveLength(1);
  expect(requestParam(requests[0], "agent_session_id")).toBe("session-a");
  expect(requestParam(requests[0], "session_start_source")).toBe("select");
  expect(requestParam(requests[0], "seq")).toBeUndefined();
});

test("reports the shuvcode identity from the shuvcode config root", async () => {
  process.env.OPENCODE_CONFIG_DIR = "/home/user/.config/shuvcode";
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "session-a" });
  await plugin.tui(tui.api);

  const dispatched = waitForNextRequest();
  tui.select("session-a");
  await dispatched;

  expect(requestParam(requests[0], "source")).toBe("herdr:shuvcode");
  expect(requestParam(requests[0], "agent")).toBe("shuvcode");
});

test("retries an initial selection while Herdr detects the process", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "session-a" });
  tui.select("session-a");

  await plugin.tui(tui.api);
  await new Promise((resolve) => setTimeout(resolve, 125));

  expect(requests.map((request) => requestParam(request, "agent_session_id"))).toEqual([
    "session-a",
    "session-a",
  ]);
});

test("does not report root sessions not selected by this TUI", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "session-a" });
  tui.addSession({ id: "session-b" });
  tui.select("session-a");
  await plugin.tui(tui.api);

  await new Promise((resolve) => setTimeout(resolve, 125));

  expect(requests.length).toBeGreaterThan(0);
  expect(requests.every((request) => requestParam(request, "agent_session_id") === "session-a")).toBe(
    true,
  );
});

test("does not replace the root session with a selected child session", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "root-session" });
  tui.addSession({ id: "child-session", parentID: "root-session" });
  tui.select("root-session");
  await plugin.tui(tui.api);
  expect(requests).toHaveLength(1);

  tui.select("child-session");
  await new Promise((resolve) => setTimeout(resolve, 125));

  expect(requests).toHaveLength(1);
  expect(requestParam(requests[0], "agent_session_id")).toBe("root-session");
});

test("stops route polling when the TUI plugin is disposed", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "session-a" });
  await plugin.tui(tui.api);
  tui.dispose();
  tui.select("session-a");

  await new Promise((resolve) => setTimeout(resolve, 125));

  expect(requests).toHaveLength(0);
});

function requestParam(request: unknown, name: string): unknown {
  if (!isRecord(request) || !isRecord(request.params)) {
    return undefined;
  }
  return request.params[name];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function v2Api() {
  const sessions = new Map([
    ["a", { id: "a" }],
    ["b", { id: "b" }],
    ["child", { id: "child", parentID: "a" }],
  ]);
  let route = { type: "session", sessionID: "a" };
  const listeners = new Set<(event: unknown) => void>();
  const permissions = new Map<string, Array<{ id: string }> | undefined>();
  const forms = new Map<string, Array<{ id: string }> | undefined>();
  const statuses = new Map<string, string>();
  const serverPermissions = new Map<string, Array<{ id: string }>>();
  const invalidated: string[] = [];
  const synced: string[] = [];
  return {
    api: {
      ui: { router: { current: () => route } },
      data: {
        session: {
          get: (id: string) => sessions.get(id),
          family: () => [...sessions.keys()],
           status: (id: string) => statuses.get(id) ?? "idle",
           permission: {
             list: (id: string) => permissions.get(id),
             invalidate: (id: string) => { invalidated.push(id); },
             sync: async (id: string) => {
               synced.push(id);
               permissions.set(id, serverPermissions.get(id) ?? []);
             },
           },
          form: { list: (id: string) => forms.get(id) },
        },
        listen: (handler: (event: unknown) => void) => {
          listeners.add(handler);
          return () => listeners.delete(handler);
        },
      },
    },
    select(sessionID: string) { route = { type: "session", sessionID }; },
    home() { route = { type: "home", sessionID: "" }; },
    emit(type: string, data?: object) {
      for (const listener of listeners) listener({ details: { type, data } });
    },
    listeners,
    sessions,
    permissions,
    forms,
    statuses,
    serverPermissions,
    invalidated,
    synced,
  };
}

const flushReports = () => new Promise((resolve) => setTimeout(resolve, 10));
// Advance debounce deadlines without sleeping through each 1.5-second delay.
// Real polling still runs, so timer disposal and reconciliation are exercised.
const advance = async (ms: number) => {
  clockOffset += ms;
  await new Promise((resolve) => setTimeout(resolve, 120));
};
const states = () => requests.filter((r) => requestParam(r, "state") !== undefined)
  .map((r) => requestParam(r, "state"));
const metadata = () => requests.filter((r) => isRecord(r) && r.method === "pane.report_metadata");
const sessionReports = () => requests.filter((r) => isRecord(r) && r.method !== "pane.report_metadata");

test("V2 ignores events without data", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  requests.length = 0;
  expect(() => tui.emit("legacy.event")).not.toThrow();
  tui.emit("session.execution.started", { sessionID: "a" });
  await flushReports();
  expect(states()).toEqual(["working"]);
});

test("V2 completes and interrupts without legacy idle events", async () => {
  for (const terminal of ["succeeded", "interrupted", "failed"]) {
    const plugin = await loadPlugin();
    const tui = v2Api();
    const dispose = await plugin.setup(tui.api);
    activeDisposers.push(dispose);
    await flushReports();
    requests.length = 0;
    tui.emit("session.execution.started", { sessionID: "a" });
    tui.emit(`session.execution.${terminal}`, { sessionID: "a" });
    await flushReports();
    expect(states()).toEqual(["working"]);
    await advance(1_600);
    expect(states().at(-1)).toBe("idle");
    expect(requestParam(metadata().at(-1), "state_labels")).toEqual(
      terminal === "failed" ? { idle: "failed", done: "failed" } : undefined,
    );
    dispose();
  }
});

test("V2 aggregates root and child blockers and ignores other roots and child completion", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  requests.length = 0;
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("permission.asked", { sessionID: "a", id: "permission-a" });
  tui.emit("form.created", { form: { sessionID: "child", id: "form-child" } });
  tui.emit("permission.replied", { sessionID: "a", requestID: "permission-a" });
  tui.emit("session.execution.succeeded", { sessionID: "child" });
  tui.emit("session.execution.started", { sessionID: "b" });
  tui.emit("permission.asked", { sessionID: "b", id: "other" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  expect(sessionReports().every((r) => requestParam(r, "agent_session_id") === "a")).toBe(true);
  tui.emit("form.cancelled", { sessionID: "child", id: "form-child" });
  tui.emit("session.execution.succeeded", { sessionID: "a" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  await advance(1_600);
  expect(states().at(-1)).toBe("idle");
});

test("V2 discards queued reports after selection changes and stops on disposal", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  requests.length = 0;
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.select("b");
  tui.emit("session.execution.started", { sessionID: "b" });
  await flushReports();
  expect(sessionReports().every((r) => requestParam(r, "agent_session_id") === "b")).toBe(true);
  requests.length = 0;
  tui.emit("session.execution.succeeded", { sessionID: "b" });
  tui.home();
  await flushReports();
  expect(requests).toHaveLength(0);
  dispose();
  expect(tui.listeners.size).toBe(0);
  tui.select("a");
  await new Promise((resolve) => setTimeout(resolve, 250));
  expect(requests).toHaveLength(0);
});

test("V2 reconciles late blocker hydration without reviving an already-replied request", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  tui.permissions.set("child", [{ id: "late" }]);
  await waitForStateReport();
  expect(states().at(-1)).toBe("blocked");
  tui.emit("permission.replied", { sessionID: "child", requestID: "late" });
  await waitForStateReport();
  expect(states().at(-1)).toBe("idle");
  tui.permissions.set("child", []);
  tui.forms.set("child", [{ id: "second" }]);
  await waitForStateReport();
  expect(states().at(-1)).toBe("blocked");
  tui.sessions.delete("child");
  tui.emit("session.deleted", { sessionID: "child" });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
});

test("V2 never writes a delayed connection after disposal or a session switch", async () => {
  for (const action of ["dispose", "switch"]) {
    const plugin = await loadPlugin();
    const tui = v2Api();
    holdConnections = true;
    requests.length = 0;
    const dispose = await plugin.setup(tui.api);
    activeDisposers.push(dispose);
    await flushReports();
    expect(connections.length).toBeGreaterThan(0);
    if (action === "dispose") dispose();
    else tui.select("b");
    holdConnections = false;
    for (const connect of connections.splice(0)) connect();
    await flushReports();
    expect(requests).toHaveLength(0);
    dispose();
  }
});

test("V2 settles a connection that never completes", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  holdConnections = true;
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  const started = Date.now();
  while (connections.length <= 1 && Date.now() - started < 2_000) {
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  expect(connections.length).toBeGreaterThan(1);
  dispose();
});

test("V2 resends the latest state after a failed delivery", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  // Exhaust the selection retry schedule so only the event report remains.
  await new Promise((resolve) => setTimeout(resolve, 1_600));
  requests.length = 0;
  tui.emit("session.execution.started", { sessionID: "a" });
  await flushReports();
  failConnections = true;
  tui.emit("session.execution.succeeded", { sessionID: "a" });
  await advance(1_600);
  await new Promise((resolve) => setTimeout(resolve, 600));
  failConnections = false;
  const resend = waitForStateReport();
  await resend;
  expect(states().at(-1)).toBe("idle");
  dispose();
});

async function startV2(tui = v2Api()) {
  const plugin = await loadPlugin();
  const dispose = plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  return { tui, dispose };
}

test("regression: interrupted permission without permission.replied returns to idle", async () => {
  const { tui } = await startV2();
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.permissions.set("a", [{ id: "ghost" }]);
  tui.emit("permission.asked", { sessionID: "a", id: "ghost" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  tui.emit("session.execution.interrupted", { sessionID: "a", reason: "user" });
  await advance(1_600);
  expect(states().at(-1)).toBe("idle");
  expect(tui.invalidated).toContain("a");
  expect(tui.synced).toContain("a");
  expect(tui.permissions.get("a")).toEqual([]);
});

test("regression: replied permissions stay expired through stale terminal sync and preserve idle hold", async () => {
  for (const cacheDropsReply of [false, true]) {
    for (const terminal of ["succeeded", "interrupted", "failed"]) {
      const { tui, dispose } = await startV2();
      tui.emit("session.execution.started", { sessionID: "a" });
      tui.permissions.set("a", [{ id: "replied" }]);
      tui.emit("permission.asked", { sessionID: "a", id: "replied" });
      await flushReports();
      if (cacheDropsReply) tui.permissions.set("a", []);
      tui.emit("permission.replied", { sessionID: "a", requestID: "replied" });
      await advance(100);
      expect(states().at(-1)).toBe("working");
      requests.length = 0;
      let finishSync: () => void = () => {};
      tui.api.data.session.permission.sync = async (id) => {
        tui.synced.push(id);
        await new Promise<void>((resolve) => { finishSync = resolve; });
        tui.permissions.set(id, [{ id: "replied" }]);
      };
      tui.emit(`session.execution.${terminal}`, { sessionID: "a" });
      await advance(100);
      finishSync();
      await flushReports();
      expect(tui.permissions.get("a")).toEqual([{ id: "replied" }]);
      expect(states()).not.toContain("blocked");
      expect(states()).not.toContain("idle");
      await advance(900);
      expect(states()).not.toContain("idle");
      await advance(700);
      expect(states().at(-1)).toBe("idle");
      expect(states()).not.toContain("blocked");
      dispose();
    }
  }
});

test("regression: settle expires cache-only permission rows before blind sync replacement", async () => {
  const { tui } = await startV2();
  tui.emit("session.execution.started", { sessionID: "a" });
  // The pre-event reconciliation reads no rows; settle then sees a newly hydrated row.
  let reads = 0;
  tui.api.data.session.permission.list = (id) => id === "a" && ++reads > 1 ? [{ id: "cache-only" }] : [];
  tui.serverPermissions.set("a", [{ id: "cache-only" }]);
  requests.length = 0;
  tui.emit("session.execution.succeeded", { sessionID: "a" });
  await advance(100);
  expect(states()).not.toContain("blocked");
  expect(states()).not.toContain("idle");
  await advance(1_600);
  expect(states().at(-1)).toBe("idle");
});

test("regression: overlapping terminal refreshes keep tombstones until every sync settles", async () => {
  for (const laterOutcome of ["success", "throw", "reject"]) {
    const { tui, dispose } = await startV2();
    tui.emit("session.execution.started", { sessionID: "a" });
    tui.permissions.set("a", [{ id: "replied" }]);
    tui.emit("permission.asked", { sessionID: "a", id: "replied" });
    await flushReports();
    tui.permissions.set("a", []);
    tui.emit("permission.replied", { sessionID: "a", requestID: "replied" });
    await flushReports();
    requests.length = 0;
    const finish: Array<() => void> = [];
    let calls = 0;
    tui.api.data.session.permission.sync = (id) => {
      const call = calls++;
      if (call === 1 && laterOutcome === "throw") throw new Error("synchronous sync failure");
      if (call === 1 && laterOutcome === "reject") return Promise.reject(new Error("async sync failure"));
      return new Promise<void>((resolve) => {
        finish.push(() => {
          // Each sync blindly replaces the cache when its own fetch completes.
          tui.permissions.set(id, call === 0 ? [{ id: "replied" }] : []);
          resolve();
        });
      });
    };
    tui.emit("session.execution.succeeded", { sessionID: "a" });
    tui.emit("session.execution.interrupted", { sessionID: "a" });
    expect(calls).toBe(2);
    if (laterOutcome === "success") finish[1]();
    // The newest sync has settled (or failed), but the older one is still in flight.
    await advance(100);
    expect(tui.permissions.get("a")).toEqual([]);
    finish[0]();
    await flushReports();
    expect(tui.permissions.get("a")).toEqual([{ id: "replied" }]);
    expect(states()).not.toContain("blocked");
    expect(states()).not.toContain("idle");
    await advance(900);
    expect(states()).not.toContain("idle");
    await advance(700);
    expect(states().at(-1)).toBe("idle");
    expect(states()).not.toContain("blocked");
    // Once all syncs have settled and the cache drops the old row, new asks work.
    tui.permissions.set("a", []);
    await advance(100);
    tui.permissions.set("a", [{ id: "new" }]);
    await advance(100);
    expect(states().at(-1)).toBe("blocked");
    dispose();
  }
});

test("regression: earlier running sibling cannot prevent later execution delta reconciliation", async () => {
  const tui = v2Api();
  tui.sessions.set("later", { id: "later", parentID: "a" });
  await startV2(tui);
  tui.statuses.set("child", "running");
  tui.emit("session.execution.started", { sessionID: "later" });
  tui.statuses.set("later", "running");
  await advance(100);
  // Miss the later sibling's terminal event while the earlier sibling is active.
  tui.statuses.set("later", "idle");
  await advance(100);
  tui.statuses.set("child", "idle");
  await advance(100);
  await advance(1_600);
  expect(states().at(-1)).toBe("idle");
});

test("V2 follows live TUI autoaccept settings on and off without restarting", async () => {
  const { tui } = await startV2();
  await writeFile(`${configDir}/cli.json`, '{"session":{"permissions":"autoaccept"}}');
  await advance(100);
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("permission.asked", { sessionID: "a", id: "auto" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  await advance(100);
  expect(states()).not.toContain("blocked");
  // The host saves config with an atomic rename, not an in-place write.
  await writeFile(`${configDir}/cli.json.tmp`, '{"session":{"permissions":"prompt"}}');
  await rename(`${configDir}/cli.json.tmp`, `${configDir}/cli.json`);
  await advance(100);
  expect(states().at(-1)).toBe("blocked");
  tui.emit("permission.replied", { sessionID: "a", requestID: "auto" });
  tui.emit("permission.asked", { sessionID: "a", id: "prompt" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
});

test("V2 recognizes all effective CLI auto aliases", async () => {
  for (const flag of ["--auto", "--yolo", "--dangerously-skip-permissions"]) {
    process.argv = [...originalArgv, flag];
    const { tui, dispose } = await startV2();
    requests.length = 0;
    tui.emit("session.execution.started", { sessionID: "a" });
    tui.emit("permission.asked", { sessionID: "a", id: "auto" });
    await advance(100);
    expect(states()).not.toContain("blocked");
    await advance(500);
    expect(states().at(-1)).toBe("blocked");
    dispose();
  }
});

test("V2 reads live JSONC settings and honors the inline config overlay", async () => {
  await writeFile(`${configDir}/cli.json`, `{
    // Keep quoted comment tokens and trailing commas inside strings intact.
    "$schema": "https://opencode.ai/v2/cli.json",
    "theme": {"name": "escaped\\\"// /* ,}",},
    "session": {"permissions": "autoaccept",}, /* trailing comment */
  }`);
  const { tui, dispose } = await startV2();
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("permission.asked", { sessionID: "a", id: "jsonc" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  dispose();
  // Explicit inline prompt mode overrides autoaccept on disk.
  process.env.OPENCODE_CLI_CONFIG_CONTENT = '{"session":{"permissions":"prompt"}}';
  const overlay = await startV2();
  overlay.tui.emit("permission.asked", { sessionID: "a", id: "prompt" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  overlay.dispose();
  process.env.OPENCODE_CLI_CONFIG_CONTENT = '{"session":{"permissions":"autoaccept"}}';
  await writeFile(`${configDir}/cli.json`, '{"session":{"permissions":"prompt"}}');
  const inlineAuto = await startV2();
  requests.length = 0;
  inlineAuto.tui.emit("permission.asked", { sessionID: "a", id: "inline-auto" });
  await advance(100);
  expect(states()).not.toContain("blocked");
});

test("V2 config toggles cannot disable CLI-forced autoaccept in the current host", async () => {
  process.argv.push("--yolo");
  const { tui } = await startV2();
  await writeFile(`${configDir}/cli.json`, '{"session":{"permissions":"prompt"}}');
  await advance(100);
  requests.length = 0;
  tui.emit("permission.asked", { sessionID: "a", id: "forced" });
  await advance(100);
  expect(states()).not.toContain("blocked");
  await advance(500);
  expect(states().at(-1)).toBe("blocked");
});

test("V2 flags after the CLI argument terminator do not enable autoaccept", async () => {
  process.argv.push("--", "--yolo");
  const { tui } = await startV2();
  tui.emit("permission.asked", { sessionID: "a", id: "prompt" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
});

test("regression: child permission orphaned by parent interrupt returns to idle", async () => {
  const { tui } = await startV2();
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("session.execution.started", { sessionID: "child" });
  tui.permissions.set("child", [{ id: "ghost" }]);
  tui.emit("permission.asked", { sessionID: "child", id: "ghost" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  tui.emit("session.execution.interrupted", { sessionID: "child", reason: "user" });
  tui.emit("session.execution.interrupted", { sessionID: "a", reason: "user" });
  await advance(1_600);
  expect(states().at(-1)).toBe("idle");
  expect(tui.invalidated).toEqual(["child", "a"]);
  expect(tui.permissions.get("child")).toEqual([]);
});

test("regression: failed root reads done with failed label until next root start", async () => {
  const { tui } = await startV2();
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("session.execution.failed", { sessionID: "a", error: { type: "provider.rate-limit" } });
  await advance(1_600);
  expect(states().at(-1)).toBe("idle");
  expect(requestParam(metadata().at(-1), "state_labels")).toEqual({ idle: "failed", done: "failed" });
  expect(requestParam(metadata().at(-1), "source")).toBe("herdr:opencode:turn");
  expect(requestParam(metadata().at(-1), "applies_to_source")).toBe("herdr:opencode");
  // A descendant's start/completion must not erase the root's failure label.
  tui.emit("session.execution.started", { sessionID: "child" });
  tui.emit("session.execution.succeeded", { sessionID: "child" });
  await advance(1_600);
  expect(requestParam(metadata().at(-1), "state_labels")).toEqual({ idle: "failed", done: "failed" });
  tui.emit("session.execution.started", { sessionID: "a" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  expect(requestParam(metadata().at(-1), "clear_state_labels")).toBe(true);
});

test("regression: root stays working while a background child runs then goes idle", async () => {
  const { tui } = await startV2();
  tui.statuses.set("a", "running");
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.statuses.set("child", "running");
  tui.emit("session.execution.started", { sessionID: "child" });
  tui.statuses.set("a", "idle");
  tui.emit("session.execution.succeeded", { sessionID: "a" });
  await advance(2_000);
  expect(states().at(-1)).toBe("working");
  tui.statuses.set("child", "idle");
  tui.emit("session.execution.succeeded", { sessionID: "child" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  await advance(1_600);
  expect(states().at(-1)).toBe("idle");
});

test("regression: idle debounce absorbs child completion to parent wake gap", async () => {
  const { tui } = await startV2();
  tui.statuses.set("child", "running");
  tui.emit("session.execution.started", { sessionID: "child" });
  await flushReports();
  requests.length = 0;
  tui.statuses.set("child", "idle");
  tui.emit("session.execution.succeeded", { sessionID: "child" });
  await advance(300);
  expect(states()).not.toContain("idle");
  tui.emit("session.execution.started", { sessionID: "a" });
  await advance(2_000);
  expect(states()).not.toContain("idle");
  tui.emit("session.execution.succeeded", { sessionID: "a" });
  await advance(1_000);
  expect(states()).not.toContain("idle");
  await advance(600);
  expect(states().filter((s) => s === "idle")).toHaveLength(1);
});

test("regression: child-route --auto stall reports blocked immediately", async () => {
  process.argv.push("--auto");
  const tui = v2Api();
  tui.select("child");
  await startV2(tui);
  tui.statuses.set("child", "running");
  tui.emit("session.execution.started", { sessionID: "child" });
  tui.permissions.set("child", [{ id: "real-ask" }]);
  tui.emit("permission.asked", { sessionID: "child", id: "real-ask" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  expect(sessionReports().every((r) => requestParam(r, "agent_session_id") === "a")).toBe(true);
  await advance(2_000);
  expect(states().at(-1)).toBe("blocked");
});

test("V2 autoaccept hides short permission blips but reports persistent asks and forms", async () => {
  process.argv.push("--auto");
  const { tui } = await startV2();
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("permission.asked", { sessionID: "child", id: "auto" });
  await advance(200);
  expect(states()).not.toContain("blocked");
  tui.emit("permission.replied", { sessionID: "child", requestID: "auto" });
  await advance(600);
  expect(states()).not.toContain("blocked");
  tui.emit("permission.asked", { sessionID: "a", id: "persistent" });
  await advance(600);
  expect(states().at(-1)).toBe("blocked");
  tui.emit("permission.replied", { sessionID: "a", requestID: "persistent" });
  tui.emit("form.created", { form: { sessionID: "a", id: "question" } });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
});

test("V2 an autoaccept permission becomes immediately blocked on a child route", async () => {
  process.argv.push("--auto");
  const { tui } = await startV2();
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("permission.asked", { sessionID: "child", id: "auto" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  tui.select("child");
  await advance(100);
  expect(states().at(-1)).toBe("blocked");
});

test("V2 expired asks cannot revive from stale or unhydrated cache without sync helpers", async () => {
  const tui = v2Api();
  // Older API shapes and existing test fakes lack these optional methods.
  delete (tui.api.data.session.permission as Partial<typeof tui.api.data.session.permission>).sync;
  delete (tui.api.data.session.permission as Partial<typeof tui.api.data.session.permission>).invalidate;
  await startV2(tui);
  tui.emit("permission.asked", { sessionID: "child", id: "ghost" });
  tui.emit("session.execution.interrupted", { sessionID: "child" });
  await advance(200);
  expect(states().at(-1)).toBe("idle");
  tui.permissions.set("child", [{ id: "ghost" }]);
  await advance(2_000);
  expect(states().at(-1)).toBe("idle");
  tui.permissions.set("child", []);
  await advance(100);
  tui.permissions.set("child", [{ id: "new" }]);
  await advance(100);
  expect(states().at(-1)).toBe("blocked");
});

test("V2 reconciles descendant activity from the cache even without lifecycle events", async () => {
  const { tui } = await startV2();
  tui.statuses.set("child", "running");
  await advance(100);
  expect(states().at(-1)).toBe("working");
  tui.statuses.set("child", "idle");
  await advance(100);
  expect(states().at(-1)).toBe("working");
  await advance(1_600);
  expect(states().at(-1)).toBe("idle");
});

test("V2 reports a failed label once per failure across later state flips", async () => {
  const { tui } = await startV2();
  for (let i = 0; i < 3; i++) await advance(1_600);
  requests.length = 0;
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("session.execution.failed", { sessionID: "a" });
  await advance(1_600);
  tui.statuses.set("child", "running");
  await advance(100);
  tui.statuses.set("child", "idle");
  await advance(100);
  await advance(1_600);
  expect(states().slice(-3)).toEqual(["idle", "working", "idle"]);
  expect(metadata().filter((r) => requestParam(r, "state_labels") !== undefined)).toHaveLength(1);
  expect(metadata().filter((r) => requestParam(r, "clear_state_labels") === true)).toHaveLength(1);
});

test("V2 restarts the idle debounce after cache-only activity resumes", async () => {
  const { tui } = await startV2();
  for (let i = 0; i < 3; i++) await advance(1_600);
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("session.execution.succeeded", { sessionID: "a" });
  await advance(100);
  tui.statuses.set("child", "running");
  await advance(2_000);
  expect(states().at(-1)).toBe("working");
  tui.statuses.set("child", "idle");
  await advance(100);
  expect(states().at(-1)).toBe("working");
  await advance(1_600);
  expect(states().at(-1)).toBe("idle");
});

test("V2 failed parent remains working while descendants run without losing its label", async () => {
  const { tui } = await startV2();
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.statuses.set("child", "running");
  tui.emit("session.execution.failed", { sessionID: "a" });
  await advance(2_000);
  expect(states().at(-1)).toBe("working");
  expect(requestParam(metadata().at(-1), "state_labels")).toEqual({ idle: "failed", done: "failed" });
  tui.statuses.set("child", "idle");
  tui.emit("session.execution.failed", { sessionID: "child" });
  await advance(1_600);
  expect(states().at(-1)).toBe("idle");
  expect(requestParam(metadata().at(-1), "state_labels")).toEqual({ idle: "failed", done: "failed" });
});

test("V2 child lifecycle deltas bridge late cache updates without pinning finished work", async () => {
  const { tui } = await startV2();
  tui.emit("session.execution.started", { sessionID: "child" });
  await advance(2_000);
  expect(states().at(-1)).toBe("working");
  tui.statuses.set("child", "running");
  await advance(100);
  tui.emit("session.execution.succeeded", { sessionID: "child" });
  // The cache still says running after the terminal event.
  await advance(1_600);
  expect(states().at(-1)).toBe("idle");
  tui.statuses.set("child", "idle");
  await advance(100);
  // A later run visible only through cache must not be suppressed by old deltas.
  tui.statuses.set("child", "running");
  await advance(100);
  expect(states().at(-1)).toBe("working");
});

test("V2 sync rejection cannot revive expired asks or interfere with another selection", async () => {
  const tui = v2Api();
  tui.api.data.session.permission.sync = async () => { throw new Error("offline"); };
  await startV2(tui);
  tui.permissions.set("a", [{ id: "ghost" }]);
  tui.emit("permission.asked", { sessionID: "a", id: "ghost" });
  tui.emit("session.execution.failed", { sessionID: "a" });
  await advance(2_000);
  expect(states().at(-1)).toBe("idle");
  tui.select("b");
  tui.emit("permission.asked", { sessionID: "b", id: "new" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  expect(requestParam(metadata().at(-1), "clear_state_labels")).toBe(true);
});

test("V2 syncs missing ancestors on a child route without repeated concurrent fetches", async () => {
  const tui = v2Api();
  tui.sessions.delete("a");
  tui.select("child");
  let resolveSync: () => void = () => {};
  const sync = mock(() => new Promise<void>((resolve) => { resolveSync = resolve; }));
  Object.assign(tui.api.data.session, { sync });
  await startV2(tui);
  await advance(500);
  expect(sync).toHaveBeenCalledTimes(1);
  expect(sync).toHaveBeenCalledWith("a");
  expect(requests).toHaveLength(0);
  tui.sessions.set("a", { id: "a" });
  tui.permissions.set("child", [{ id: "real" }]);
  resolveSync();
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  expect(sessionReports().every((r) => requestParam(r, "agent_session_id") === "a")).toBe(true);
});

test("V2 debounce state and failed metadata do not leak across selections or disposal", async () => {
  const { tui, dispose } = await startV2();
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("session.execution.failed", { sessionID: "a" });
  await flushReports();
  requests.length = 0;
  tui.select("b");
  await advance(200);
  expect(states().at(-1)).toBe("idle");
  expect(requestParam(metadata().at(-1), "clear_state_labels")).toBe(true);
  expect(sessionReports().every((r) => requestParam(r, "agent_session_id") === "b")).toBe(true);
  dispose();
  requests.length = 0;
  await advance(2_000);
  expect(requests).toHaveLength(0);
});

test("V2 opt-in publish trace contains reasons and owners but no prompt payload", async () => {
  await mkdir(".local", { recursive: true });
  const path = `.local/herdr-tui-trace-${process.pid}.jsonl`;
  process.env.HERDR_OPENCODE_TRACE = path;
  const { tui, dispose } = await startV2();
  try {
    tui.emit("permission.asked", { sessionID: "child", id: "trace", secretPrompt: "never-record-me" });
    await flushReports();
    const text = await readFile(path, "utf8");
    const entries = text.trim().split("\n").map((line) => JSON.parse(line));
    expect(entries.some((e) => e.reason === "permission.asked" && e.root === "a" &&
      e.route.sessionID === "a" && e.rootState === "idle" && e.state === "blocked" &&
      e.blockers.some((b: { key: string; owner: string }) => b.key === "permission:trace" && b.owner === "child"))).toBe(true);
    expect(text).not.toContain("never-record-me");
    dispose();
  } finally {
    dispose();
    await rm(path, { force: true });
  }
});

test("V2 an unwritable trace does not prevent status delivery", async () => {
  process.env.HERDR_OPENCODE_TRACE = ".local/missing-trace-dir/trace.jsonl";
  const { tui } = await startV2();
  tui.emit("session.execution.started", { sessionID: "a" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
});
