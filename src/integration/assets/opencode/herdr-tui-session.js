// installed by herdr
// managed by herdr; reinstalling or updating the integration overwrites this file.
// HERDR_INTEGRATION_ID=opencode-tui
// HERDR_INTEGRATION_VERSION=14
// V2 TUI entrypoint herdr-opencode/tui.js re-exports this file.

import net from "node:net";
import { appendFile, readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { watch } from "node:fs";
import path from "node:path";

const AGENT = isShuvcodeHost() ? "shuvcode" : "opencode";
const SOURCE = `herdr:${AGENT}`;
const ROUTE_POLL_INTERVAL_MS = 100;
const SELECTION_RETRY_DELAYS_MS = [100, 400, 1_000];
const IDLE_DELAY_MS = 1_500;
const AUTO_BLOCKED_DELAY_MS = 500;
const CONFIG_POLL_INTERVAL_MS = 1_000;

// The host reads JSONC (comments and trailing commas) even in cli.json.
// Preserve quoted strings when removing either, including URLs and escaped quotes.
function cliPermissionMode(text) {
  if (!text) return;
  try {
    const json = text.replace(/("(?:\\.|[^"\\])*")|\/\/[^\r\n]*|\/\*[\s\S]*?\*\//g,
      (match, string) => string ?? " ").replace(/("(?:\\.|[^"\\])*")|,\s*(?=[}\]])/g,
      (match, string) => string ?? "");
    const mode = JSON.parse(json)?.session?.permissions;
    return mode === "autoaccept" || mode === "prompt" ? mode : undefined;
  } catch {
    // Invalid or unavailable config is not evidence of autoaccept.
  }
}

function isShuvcodeHost() {
  if (/(?:^|[\\/])shuvcode[\\/]plugins[\\/]/i.test(import.meta.url)) {
    return true;
  }
  const executablePattern = /(?:^|[\\/])shuvcode(?:\.(?:exe|cmd|bat|ps1))?$/i;
  if ([process.execPath, process.argv0, process.argv?.[0], process.argv?.[1]].some((value) =>
    typeof value === "string" && executablePattern.test(value)
  )) {
    return true;
  }
  const configDir = process.env.OPENCODE_CONFIG_DIR;
  return typeof configDir === "string" && /(?:^|[\\/])shuvcode[\\/]?$/i.test(configDir);
}

function requestOnce(sessionID, state, seq, isCurrent = () => true, failedLabel) {
  const paneId = process.env.HERDR_PANE_ID;
  const socketPath = process.env.HERDR_SOCKET_PATH;
  if (!paneId || !socketPath) {
    return Promise.resolve(true);
  }

  const socketEndpoint =
    process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;
  const request = {
    id: `${SOURCE}:tui:${Date.now()}:${Math.floor(Math.random() * 1_000_000)
      .toString()
      .padStart(6, "0")}`,
    method: failedLabel !== undefined ? "pane.report_metadata" : state === undefined ? "pane.report_agent_session" : "pane.report_agent",
    params: failedLabel !== undefined ? {
      pane_id: paneId,
      source: `${SOURCE}:turn`,
      agent: AGENT,
      applies_to_source: SOURCE,
      seq,
      ...(failedLabel ? { state_labels: { idle: "failed", done: "failed" } } : { clear_state_labels: true }),
    } : {
      pane_id: paneId,
      source: SOURCE,
      agent: AGENT,
      agent_session_id: sessionID,
      ...(state === undefined ? { session_start_source: "select" } : { state, seq }),
    },
  };

  return new Promise((resolve) => {
    let settled = false;
    let timer;
    const settle = (delivered) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      client.destroy();
      resolve(delivered);
    };
    const client = net.createConnection(socketEndpoint, () => {
      if (!isCurrent()) {
        settle(false);
        return;
      }
      client.write(`${JSON.stringify(request)}\n`);
    });

    // A plain timer, not socket.setTimeout, so a connection that never finishes
    // connecting still settles and cannot block later reports behind the queue.
    timer = setTimeout(() => settle(false), 500);
    timer.unref?.();
    client.on("data", () => settle(true));
    client.on("error", () => settle(false));
    client.on("end", () => settle(false));
    client.on("close", () => settle(false));
  });
}

export default {
  id: "herdr.opencode.session-selection",
  // Keep this plain object dependency-free: V1 and V2 expose different SDK
  // packages, but both loaders accept their own lifecycle entry on this object.
  setup,
  tui,
};

async function tui(api) {
  if (process.env.HERDR_ENV !== "1" || !process.env.HERDR_SOCKET_PATH || !process.env.HERDR_PANE_ID) return;

  let disposed = false;
  let context;
  let sequence = Date.now() * 1000;
  let chain = Promise.resolve();

  function routeID() {
    const route = api.route.current;
    return route?.name === "session" ? route.params?.sessionID : undefined;
  }

  function current(ctx) {
    return !disposed && context === ctx && routeID() === ctx.route;
  }

  async function read(ctx, request) {
    const result = await request({
      signal: AbortSignal.any([ctx.controller.signal, AbortSignal.timeout(5_000)]),
      throwOnError: true,
    });
    if (!current(ctx) || result?.data === undefined) throw new Error("session data unavailable");
    return result.data;
  }

  // The first selected session owns this pane's subtree. Browsing descendants
  // keeps that boundary; directly attaching to a child does not claim siblings.
  function root(ctx, id) {
    const seen = new Set();
    while (typeof id === "string" && !seen.has(id) && !ctx.deleted.has(id)) {
      seen.add(id);
      if (id === ctx.boundary) return id;
      const session = ctx.sessions.get(id) ?? api.state.session.get(id);
      if (!session) return;
      if (!session.parentID) return id;
      id = session.parentID;
    }
  }

  async function resolveRoot(ctx, id) {
    const seen = new Set();
    while (typeof id === "string" && !seen.has(id) && !ctx.deleted.has(id)) {
      seen.add(id);
      if (id === ctx.boundary) return id;
      let session = ctx.sessions.get(id) ?? api.state.session.get(id);
      if (!session) {
        if (!ctx.lookups.has(id)) {
          const requestedID = id;
          const lookup = read(ctx, (options) => api.client.session.get({ sessionID: requestedID }, options));
          ctx.lookups.set(id, lookup);
          lookup.finally(() => ctx.lookups.delete(requestedID)).catch(() => {});
        }
        session = await ctx.lookups.get(id);
        if (ctx.deleted.has(id)) return;
        session = ctx.sessions.get(id) ?? session;
        if (session.id !== id) throw new Error("unexpected session identity");
        ctx.sessions.set(id, session);
      }
      if (!session.parentID) return id;
      id = session.parentID;
    }
  }

  function owners(ctx) {
    return new Set([...ctx.statuses.keys(), ...[...ctx.blockers.values()].map((r) => r.sessionID)]);
  }

  function state(ctx) {
    if (ctx.settled) return "idle";
    for (const request of ctx.blockers.values()) {
      if (root(ctx, request.sessionID) === ctx.selected) return "blocked";
    }
    if (ctx.errors.has(ctx.selected)) return "blocked";
    for (const id of ctx.statuses.keys()) {
      if (root(ctx, id) === ctx.selected) return "working";
    }
    if (ctx.hydrated && [...owners(ctx)].every((id) => root(ctx, id) !== undefined)) return "idle";
  }

  function publish(ctx, selection = false) {
    if (!current(ctx) || !ctx.selected) return;
    ctx.selectionPending ||= selection;
    if (ctx.queued) return;
    ctx.queued = true;
    chain = chain.then(async () => {
      ctx.queued = false;
      const selected = ctx.selected;
      const isCurrent = () => current(ctx) && !!selected && ctx.selected === selected &&
        (ctx.settled || root(ctx, ctx.route) === selected);
      if (!isCurrent()) return;
      if (ctx.selectionPending) {
        if (!await requestOnce(selected, undefined, undefined, isCurrent)) {
          ctx.retryAt = Date.now() + 500;
          return;
        }
        if (!isCurrent()) return;
        ctx.selectionPending = false;
        ctx.lastState = undefined;
      }
      const value = state(ctx);
      if (value === undefined || value === ctx.lastState) return;
      const delivered = await requestOnce(selected, value, ++sequence, isCurrent);
      if (!isCurrent()) return;
      ctx.lastState = delivered ? value : undefined;
      if (!delivered) ctx.retryAt = Date.now() + 500;
    }).catch(() => {
      if (current(ctx)) {
        ctx.lastState = undefined;
        ctx.retryAt = Date.now() + 500;
      }
    });
  }

  function retire(ctx, route) {
    ctx?.controller?.abort();
    context = ctx?.selected
      ? { route, selected: ctx.selected, settled: true, retryAt: Infinity }
      : undefined;
    if (context) publish(context);
  }

  function reconcile(ctx) {
    if (!current(ctx)) return;
    publish(ctx);
    if (ctx.resolving) {
      ctx.resolveAgain = true;
      return;
    }
    ctx.resolving = true;
    void (async () => {
      const ancestor = await resolveRoot(ctx, ctx.route);
      if (!current(ctx) || !ancestor) return;
      const selected = ancestor === ctx.boundary ? ctx.boundary : ctx.route;
      if (selected !== ctx.selected) {
        ctx.boundary = selected;
        ctx.selected = selected;
        publish(ctx, true);
      }
      // Fetch ancestry only for active/request-bearing sessions, not historical trees.
      await Promise.all([...owners(ctx)].map((id) => resolveRoot(ctx, id)));
      if (current(ctx)) publish(ctx);
    })().catch(() => {
      if (current(ctx)) ctx.retryAt = Date.now() + 500;
    }).finally(() => {
      ctx.resolving = false;
      if (ctx.resolveAgain) {
        ctx.resolveAgain = false;
        reconcile(ctx);
      }
    });
  }

  function clearRequests(ctx, matches) {
    for (const [key, request] of ctx.blockers) if (matches(request)) ctx.blockers.delete(key);
  }

  function terminalTool(part) {
    return part?.type === "tool" && ["completed", "error"].includes(part.state?.status);
  }

  function apply(ctx, event) {
    const data = event.properties;
    if (!data) return;
    if (event.type === "message.part.updated") {
      const part = data.part;
      if (terminalTool(part)) {
        clearRequests(ctx, (r) => r.sessionID === part.sessionID &&
          r.tool?.messageID === part.messageID && r.tool?.callID === part.callID);
      }
      return;
    }
    const id = data.sessionID ?? data.info?.id;
    if (typeof id !== "string" || ctx.deleted.has(id)) return;
    switch (event.type) {
      case "session.created":
      case "session.updated":
        if (data.info) ctx.sessions.set(id, data.info);
        break;
      case "session.deleted":
        ctx.deleted.add(id);
        ctx.sessions.delete(id);
        ctx.statuses.delete(id);
        ctx.errors.delete(id);
        clearRequests(ctx, (r) => r.sessionID === id);
        if (ctx.selected === id) retire(ctx, ctx.route);
        break;
      case "session.status":
      case "session.idle": {
        const status = event.type === "session.idle" ? "idle" : data.status?.type;
        if (status === "busy" || status === "retry") {
          ctx.statuses.set(id, status);
          ctx.errors.delete(id);
        } else if (status === "idle") {
          ctx.statuses.delete(id);
          ctx.errors.delete(id);
        }
        break;
      }
      case "session.error":
        if (data.error?.name !== "MessageAbortedError") ctx.errors.add(id);
        break;
      case "permission.asked":
      case "question.asked":
        if (typeof data.id === "string") ctx.blockers.set(`${event.type.split(".")[0]}:${data.id}`, data);
        break;
      case "permission.replied":
      case "question.replied":
      case "question.rejected":
        ctx.blockers.delete(`${event.type.split(".")[0]}:${data.requestID}`);
        break;
    }
  }

  async function hydrate(ctx) {
    if (!current(ctx) || ctx.loading) return;
    ctx.loading = true;
    try {
      const [statuses, permissions, questions] = await Promise.all([
        read(ctx, (options) => api.client.session.status(undefined, options)),
        read(ctx, (options) => api.client.permission.list(undefined, options)),
        read(ctx, (options) => api.client.question.list(undefined, options)),
      ]);
      if (!statuses || typeof statuses !== "object" || Array.isArray(statuses) ||
          !Object.values(statuses).every((s) => ["busy", "retry", "idle"].includes(s?.type)) ||
          !Array.isArray(permissions) || !Array.isArray(questions)) {
        throw new Error("incomplete session snapshot");
      }
      const messages = new Map();
      const blockers = new Map();
      let validated = true;
      await Promise.all([["permission", permissions], ["question", questions]].flatMap(([kind, requests]) =>
        requests.map(async (request) => {
          if (ctx.deleted.has(request.sessionID)) return;
          if (request.tool) {
            const { messageID, callID } = request.tool;
            const key = `${request.sessionID}:${messageID}`;
            if (!messages.has(key)) messages.set(key, read(ctx, (options) => api.client.session.message({
              sessionID: request.sessionID, messageID,
            }, options)));
            try {
              const message = await messages.get(key);
              const part = message.parts?.find((p) => p.type === "tool" && p.callID === callID);
              // OpenCode can retain pending requests after abort. Status alone is
              // insufficient: the same session may already be running a new turn.
              if (terminalTool(part)) return;
            } catch {
              validated = false;
            }
          }
          if (!ctx.deleted.has(request.sessionID)) blockers.set(`${kind}:${request.id}`, request);
        })));
      if (!current(ctx)) return;
      ctx.statuses = new Map(Object.entries(statuses)
        .filter(([id, status]) => !ctx.deleted.has(id) && (status.type === "busy" || status.type === "retry")));
      ctx.blockers = blockers;
      // Preserve deltas until hydration succeeds, including retry backoff:
      // a later snapshot must not revive a replied or cancelled request.
      for (const event of ctx.events) apply(ctx, event);
      ctx.hydrated = validated;
      if (!validated) ctx.retryAt = Date.now() + 500;
      reconcile(ctx);
    } catch {
      if (current(ctx)) ctx.retryAt = Date.now() + 500;
    } finally {
      ctx.loading = false;
      if (ctx.hydrated) ctx.events = [];
    }
  }

  function syncSelection(reset = false) {
    if (disposed) return;
    const id = routeID();
    if (reset || id !== context?.route) {
      if (typeof id !== "string" || !id) {
        retire(context, id);
        return;
      }
      const boundary = context?.settled ? undefined : context?.selected;
      context?.controller?.abort();
      const ctx = {
        route: id, boundary, controller: new AbortController(), sessions: new Map(), lookups: new Map(),
        statuses: new Map(), blockers: new Map(), errors: new Set(), deleted: new Set(),
        events: [], hydrated: false, loading: false, resolving: false,
        retryAt: Infinity, selectionAt: 0, retryIndex: 0,
      };
      context = ctx;
      reconcile(ctx);
      void hydrate(ctx);
    }
    const ctx = context;
    if (!ctx) return;
    if (!ctx.settled && ctx.selected && Date.now() >= ctx.selectionAt) {
      publish(ctx, true);
      const delay = SELECTION_RETRY_DELAYS_MS[ctx.retryIndex++];
      ctx.selectionAt = delay === undefined ? Infinity : Date.now() + delay;
    }
    if (Date.now() >= ctx.retryAt) {
      ctx.retryAt = Infinity;
      if (ctx.settled) publish(ctx);
      else {
        if (!ctx.hydrated) void hydrate(ctx);
        reconcile(ctx);
      }
    }
  }

  const subscriptions = [
    "session.created", "session.updated", "session.deleted", "session.status", "session.idle", "session.error",
    "permission.asked", "permission.replied", "question.asked", "question.replied", "question.rejected",
    "message.part.updated",
  ].map((type) => api.event.on(type, (event) => {
    if (type === "message.part.updated" && !terminalTool(event.properties?.part)) return;
    syncSelection();
    const ctx = context;
    if (!ctx || ctx.settled) return;
    if (ctx.loading || !ctx.hydrated) ctx.events.push(event);
    apply(ctx, event);
    reconcile(ctx);
  }));
  for (const type of ["server.connected", "server.instance.disposed", "global.disposed"]) {
    subscriptions.push(api.event.on(type, () => syncSelection(true)));
  }
  const poll = setInterval(syncSelection, ROUTE_POLL_INTERVAL_MS);
  api.lifecycle.onDispose(() => {
    disposed = true;
    context?.controller?.abort();
    clearInterval(poll);
    for (const unsubscribe of subscriptions) unsubscribe();
  });
  syncSelection();
}

function setup(api) {
  if (process.env.HERDR_ENV !== "1" || !process.env.HERDR_SOCKET_PATH || !process.env.HERDR_PANE_ID) return;

  let disposed = false;
  let selected;
  let generation = 0;
  let sequence = Date.now() * 1000;
  let chain = Promise.resolve();
  let retryIndex = 0;
  let nextSelectionAt = 0;
  let state = "idle";
  let failed = false;
  let published;
  let idleAt;
  let blockedAt;
  let retryTimer;
  const sessions = new Map();
  const ancestorSyncs = new Map();
  const executionChanges = new Map();
  const permissionSyncs = new Map();
  // Context exposes neither app permissions.autoApprove, TUI config nor args.auto.
  // Best available live TUI signal: persisted session.permissions + inline overlay.
  // CLI aliases force autoaccept in usePermission, even when the config says prompt.
  const args = process.argv.slice(2);
  const cliAuto = args.slice(0, args.indexOf("--") < 0 ? args.length : args.indexOf("--"))
    .some((arg) => ["--auto", "--yolo", "--dangerously-skip-permissions"].includes(arg));
  const configPath = path.join(process.env.OPENCODE_CONFIG_DIR ||
    path.join(process.env.XDG_CONFIG_HOME || path.join(homedir(), ".config"), AGENT), "cli.json");
  const inlineMode = cliPermissionMode(process.env.OPENCODE_CLI_CONFIG_CONTENT);
  let autoAccept = cliAuto || inlineMode === "autoaccept";
  let modePending = false;
  let modeWatcher;
  let modeDirty = true;
  let nextModeReadAt = 0;
  let blockers = new Map();
  // Keep dead permission keys suppressed until their owner's cache drops them.
  const expired = new Map();
  // Event callbacks may precede cache updates. Retain each delta until the
  // cache reflects it, so late hydration cannot undo a reply or lose an ask.
  const blockerChanges = new Map();

  function root(id) {
    const seen = new Set();
    while (typeof id === "string" && !seen.has(id)) {
      seen.add(id);
      const session = api.data.session.get(id) ?? sessions.get(id);
      if (!session) {
        syncAncestor(id);
        return;
      }
      if (!session.parentID) return id;
      id = session.parentID;
    }
  }

  function syncAncestor(id) {
    if (disposed || typeof api.data.session.sync !== "function" || Date.now() < (ancestorSyncs.get(id) ?? 0)) return;
    ancestorSyncs.set(id, Number.POSITIVE_INFINITY);
    try {
      void Promise.resolve(api.data.session.sync(id)).catch(() => {}).finally(() => {
        if (disposed) return;
        ancestorSyncs.set(id, Date.now() + 1_000);
        syncSelection();
      });
    } catch {
      ancestorSyncs.set(id, Date.now() + 1_000);
    }
  }

  function current() {
    const route = api.ui.router.current();
    return route.type === "session" ? root(route.sessionID) : undefined;
  }

  // Selection and lifecycle use one queue. Recheck attribution at dispatch,
  // not just when receiving the event, and reject A -> B -> A stale work too.
  function enqueue(value, failedLabel) {
    const sessionID = selected;
    const revision = generation;
    const isCurrent = () => !disposed && revision === generation && !!sessionID && current() === sessionID;
    chain = chain.then(async () => {
      if (!isCurrent()) return;
      const delivered = await requestOnce(sessionID, value, value === undefined && failedLabel === undefined ? undefined : ++sequence, isCurrent, failedLabel);
      if (!delivered) scheduleStateRetry();
    }).catch(() => {});
  }

  // A dropped report must not strand the pane on a stale state once the
  // selection retry schedule has run out: resend the latest state until the
  // socket accepts it or the selection is no longer current.
  function scheduleStateRetry() {
    if (disposed || retryTimer) return;
    retryTimer = setTimeout(() => {
      retryTimer = undefined;
      publish("delivery-retry", true);
    }, 500);
    retryTimer.unref?.();
  }

  function familyActive() {
    let active = false;
    for (const member of api.data.session.family(selected)) {
      if (member === selected || root(member) !== selected) continue;
      const running = api.data.session.status(member) === "running";
      if (executionChanges.get(member) === running) executionChanges.delete(member);
      if (executionChanges.get(member) ?? running) active = true;
    }
    return active;
  }

  function refreshPermissionMode() {
    if (disposed || modePending || cliAuto || inlineMode !== undefined) return;
    // File events refresh immediately; a 1s safety poll covers missed events or
    // directory replacement without reading config on every 100ms route tick.
    if (!modeDirty && Date.now() < nextModeReadAt) return;
    modeDirty = false;
    nextModeReadAt = Date.now() + CONFIG_POLL_INTERVAL_MS;
    modePending = true;
    void readFile(configPath, "utf8").catch(() => undefined).then((text) => {
      if (disposed) return;
      const next = cliPermissionMode(text) === "autoaccept";
      if (next === autoAccept) return;
      autoAccept = next;
      if (selected) publish("permission-mode");
    }).finally(() => { modePending = false; });
  }

  function finishPermissionSync(member, token, revision) {
    if (disposed || revision !== generation) return false;
    const pending = permissionSyncs.get(member);
    pending?.delete(token);
    if (pending?.size === 0) permissionSyncs.delete(member);
    return !!selected;
  }

  function effective() {
    if (blockers.size) return "blocked";
    return state === "working" || familyActive() ? "working" : "idle";
  }

  function publish(reason, force = false) {
    const raw = effective();
    let value = raw;
    const route = api.ui.router.current();
    // Only root-route autoaccept permissions are expected to resolve themselves.
    // Forms, prompt mode, and child-route autoaccept stalls need immediate attention.
    if (raw === "blocked" && autoAccept && route.sessionID === selected &&
      [...blockers.keys()].every((key) => key.startsWith("permission:")) && published !== "blocked") {
      blockedAt ??= Date.now() + AUTO_BLOCKED_DELAY_MS;
      if (Date.now() < blockedAt) value = state === "working" || familyActive() ? "working" : "idle";
    } else {
      blockedAt = undefined;
    }
    if (raw === "idle" && published === "working") {
      idleAt ??= Date.now() + IDLE_DELAY_MS;
      if (Date.now() < idleAt) value = "working";
    } else {
      idleAt = undefined;
    }
    const changed = published !== value;
    published = value;
    // Opt-in, content-free diagnostics; trace I/O must never delay status delivery.
    if (process.env.HERDR_OPENCODE_TRACE) {
      const entry = { t: Date.now(), reason, route: { type: route.type, sessionID: route.sessionID }, root: selected, rootState: state,
        failed, blockers: [...blockers].map(([key, owner]) => ({ key, owner })), raw, state: value };
      void appendFile(process.env.HERDR_OPENCODE_TRACE, `${JSON.stringify(entry)}\n`).catch(() => {});
    }
    if (changed || force) enqueue(value);
    // Retrying this source separately also retries a dropped label/clear report.
    if (force) enqueue(undefined, failed);
  }

  function settleMember(member) {
    for (const [key, owner] of blockers) {
      if (owner === member && key.startsWith("permission:")) expired.set(key, owner);
    }
    for (const [key, change] of blockerChanges) {
      if (change.id === member && change.kind === "permission") {
        // Replies are tombstones too: a blind sync can still return their IDs.
        expired.set(key, member);
        blockerChanges.delete(key);
      }
    }
    const permission = api.data.session.permission;
    for (const item of permission.list(member) ?? []) expired.set(`permission:${item.id}`, member);
    const revision = generation;
    let token;
    try {
      if (typeof permission.invalidate === "function") permission.invalidate(member);
      if (typeof permission.sync === "function") {
        token = {};
        const pending = permissionSyncs.get(member) ?? new Set();
        pending.add(token);
        permissionSyncs.set(member, pending);
        void Promise.resolve(permission.sync(member)).then(() => {
          if (finishPermissionSync(member, token, revision)) {
            reconcileBlockers();
            publish("permission-sync");
          }
        }, () => {
          finishPermissionSync(member, token, revision);
        });
      }
    } catch {
      if (token) finishPermissionSync(member, token, revision);
      // Best effort: expired keys already hide the dead asks.
    }
    reconcileBlockers();
  }

  function changeBlocker(id, kind, requestID, present) {
    if (typeof requestID !== "string") return;
    const key = `${kind}:${requestID}`;
    if (present) expired.delete(key);
    blockerChanges.set(key, { id, kind, present });
    if (present) blockers.set(key, id);
    else blockers.delete(key);
  }

  function reconcileBlockers() {
    const next = new Map();
    const hydrated = new Set();
    const cached = new Set();
    const members = new Set([selected, ...api.data.session.family(selected), ...blockers.values(), ...expired.values()]);
    for (const member of members) {
      if (root(member) !== selected) continue;
      for (const kind of ["permission", "form"]) {
        const items = api.data.session[kind].list(member);
        if (items === undefined) {
          for (const [key, owner] of blockers) {
            if (owner === member && key.startsWith(`${kind}:`) && !expired.has(key)) next.set(key, owner);
          }
          continue;
        }
        hydrated.add(`${kind}:${member}`);
        for (const item of items) {
          const key = `${kind}:${item.id}`;
          cached.add(key);
          if (!expired.has(key)) next.set(key, member);
        }
      }
    }
    for (const [key, change] of blockerChanges) {
      if (hydrated.has(`${change.kind}:${change.id}`) && next.has(key) === change.present &&
        (change.kind !== "permission" || change.present || expired.has(key))) {
        blockerChanges.delete(key);
      } else if (change.present) {
        next.set(key, change.id);
      } else {
        next.delete(key);
      }
    }
    for (const [key, owner] of expired) {
      // A pending refresh can blindly replace an empty cache with stale rows.
      if (!permissionSyncs.has(owner) && hydrated.has(`permission:${owner}`) && !cached.has(key)) expired.delete(key);
    }
    blockers = next;
  }

  function syncSelection() {
    if (disposed) return;
    const id = current();
    if (id !== selected) {
      selected = id;
      generation += 1;
      retryIndex = 0;
      nextSelectionAt = 0;
      blockers.clear();
      blockerChanges.clear();
      expired.clear();
      executionChanges.clear();
      permissionSyncs.clear();
      failed = false;
      published = undefined;
      idleAt = undefined;
      blockedAt = undefined;
      if (id) {
        state = api.data.session.status(id) === "running" ? "working" : "idle";
      }
    }
    if (!id) return;
    reconcileBlockers();
    const raw = effective();
    if (raw !== "idle") idleAt = undefined;
    if (raw !== "blocked") blockedAt = undefined;
    if (Date.now() < nextSelectionAt) {
      if (raw !== published) publish("reconcile");
      return;
    }
    enqueue(undefined);
    publish("selection", true);
    const delay = SELECTION_RETRY_DELAYS_MS[retryIndex++];
    nextSelectionAt = delay === undefined ? Number.POSITIVE_INFINITY : Date.now() + delay;
  }

  function receive({ details: event }) {
    if (disposed) return;
    const data = event.data;
    if (data == null) return;
    if (event.type === "session.created") {
      sessions.set(data.sessionID, { id: data.sessionID, parentID: data.parentID });
    }
    if (event.type === "session.deleted") {
      const affected = data.sessionID === selected || [...blockers.values()].includes(data.sessionID);
      sessions.delete(data.sessionID);
      executionChanges.delete(data.sessionID);
      permissionSyncs.delete(data.sessionID);
      // Deletion is delivered after the cache can remove the session. Use
      // stored ownership rather than looking up the deleted child's ancestry.
      for (const [key, owner] of blockers) if (owner === data.sessionID) blockers.delete(key);
      for (const [key, change] of blockerChanges) {
        if (change.id === data.sessionID) blockerChanges.delete(key);
      }
      for (const [key, owner] of expired) if (owner === data.sessionID) expired.delete(key);
      syncSelection();
      if (selected && affected) publish(event.type);
      return;
    }
    syncSelection();
    const id = event.type === "form.created" ? data.form.sessionID : data.sessionID;
    if (!selected || root(id) !== selected) return;
    switch (event.type) {
      case "permission.asked":
        changeBlocker(id, "permission", data.id, true);
        break;
      case "permission.replied":
        changeBlocker(id, "permission", data.requestID, false);
        break;
      case "form.created":
        changeBlocker(id, "form", data.form.id, true);
        break;
      case "form.replied":
      case "form.cancelled":
        changeBlocker(id, "form", data.id, false);
        break;
      case "session.execution.started":
        executionChanges.set(id, true);
        if (id === selected) {
          state = "working";
          failed = false;
          enqueue(undefined, false);
        }
        break;
      case "session.execution.succeeded":
      case "session.execution.interrupted":
      case "session.execution.failed":
        executionChanges.set(id, false);
        settleMember(id);
        if (id === selected) {
          state = "idle";
          if (event.type === "session.execution.failed") {
            failed = true;
            enqueue(undefined, true);
          }
        }
        break;
      default:
        return;
    }
    publish(event.type);
  }

  const unsubscribe = api.data.listen(receive);
  if (!cliAuto && inlineMode === undefined) {
    try {
      // Watch the directory to follow atomic cli.json replacements, like the host.
      // Runtimes may report a rename under the temp file's name, so any entry
      // change in the directory rereads the config.
      modeWatcher = watch(path.dirname(configPath), () => {
        modeDirty = true;
        refreshPermissionMode();
      });
      modeWatcher.on("error", () => {
        modeWatcher?.close();
        modeWatcher = undefined;
      });
      modeWatcher.unref?.();
    } catch {
      // Missing directories or unavailable watchers use the throttled poll.
    }
  }
  refreshPermissionMode();
  syncSelection();
  const poll = setInterval(() => {
    refreshPermissionMode();
    syncSelection();
  }, ROUTE_POLL_INTERVAL_MS);
  return () => {
    disposed = true;
    generation += 1;
    clearTimeout(retryTimer);
    clearInterval(poll);
    modeWatcher?.close();
    unsubscribe();
    sessions.clear();
    ancestorSyncs.clear();
    executionChanges.clear();
    permissionSyncs.clear();
    blockers.clear();
    blockerChanges.clear();
    expired.clear();
  };
}
