// installed by herdr
// managed by herdr; reinstalling or updating the integration overwrites this file.
// HERDR_INTEGRATION_ID=opencode-tui
// HERDR_INTEGRATION_VERSION=13
// V2 TUI entrypoint herdr-opencode/tui.js re-exports this file.

import net from "node:net";
import { appendFile } from "node:fs/promises";

const AGENT = isShuvcodeHost() ? "shuvcode" : "opencode";
const SOURCE = `herdr:${AGENT}`;
const ROUTE_POLL_INTERVAL_MS = 100;
const SELECTION_RETRY_DELAYS_MS = [100, 400, 1_000];
const IDLE_DELAY_MS = 1_500;
const AUTO_BLOCKED_DELAY_MS = 500;

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
  tui: async (api) => {
    if (
      process.env.HERDR_ENV !== "1" ||
      !process.env.HERDR_SOCKET_PATH ||
      !process.env.HERDR_PANE_ID
    ) {
      return;
    }

    let selectedSessionID;
    let retryIndex = 0;
    let nextReportAt = 0;
    let reportPending = false;
    const syncSelectedSession = async () => {
      const route = api.route.current;
      const sessionID = route?.name === "session" ? route.params?.sessionID : undefined;
      const session =
        typeof sessionID === "string" && sessionID
          ? api.state.session.get(sessionID)
          : undefined;
      if (!session || session.parentID) {
        selectedSessionID = undefined;
        retryIndex = 0;
        nextReportAt = 0;
        return;
      }
      if (sessionID !== selectedSessionID) {
        selectedSessionID = sessionID;
        retryIndex = 0;
        nextReportAt = 0;
      }
      if (reportPending || Date.now() < nextReportAt) {
        return;
      }

      const reportingSessionID = sessionID;
      reportPending = true;
      try {
        await requestOnce(reportingSessionID);
      } catch {
        // Best-effort reporting retries below while the selected route remains active.
      } finally {
        reportPending = false;
      }
      if (selectedSessionID !== reportingSessionID) {
        retryIndex = 0;
        nextReportAt = 0;
        return;
      }
      const retryDelay = SELECTION_RETRY_DELAYS_MS[retryIndex];
      retryIndex += 1;
      nextReportAt = retryDelay === undefined ? Number.POSITIVE_INFINITY : Date.now() + retryDelay;
    };

    await syncSelectedSession();
    const routePoll = setInterval(() => void syncSelectedSession(), ROUTE_POLL_INTERVAL_MS);
    api.lifecycle.onDispose(() => clearInterval(routePoll));
  },
};

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
    for (const member of api.data.session.family(selected)) {
      if (member === selected || root(member) !== selected) continue;
      const running = api.data.session.status(member) === "running";
      if (executionChanges.get(member) === running) executionChanges.delete(member);
      if (executionChanges.get(member) ?? running) return true;
    }
    return false;
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
    // Forms, prompt mode, and child-route --auto stalls need immediate attention.
    // V2's public plugin API has no permission-mode accessor. --auto guarantees
    // autoaccept; without it, conservatively keep prompt-mode asks immediate.
    const auto = process.argv.includes("--auto");
    if (raw === "blocked" && auto && route.sessionID === selected &&
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
    if (changed || force) enqueue(undefined, failed);
  }

  function settleMember(member) {
    for (const [key, owner] of blockers) {
      if (owner === member && key.startsWith("permission:")) expired.set(key, owner);
    }
    for (const [key, change] of blockerChanges) {
      if (change.id === member && change.kind === "permission") {
        if (change.present) expired.set(key, member);
        blockerChanges.delete(key);
      }
    }
    const permission = api.data.session.permission;
    try {
      if (typeof permission.invalidate === "function") permission.invalidate(member);
      if (typeof permission.sync === "function") {
        const revision = generation;
        void Promise.resolve(permission.sync(member)).then(() => {
          if (!disposed && revision === generation && selected) {
            reconcileBlockers();
            publish("permission-sync");
          }
        }, () => {});
      }
    } catch {
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
      if (hydrated.has(`${change.kind}:${change.id}`) && next.has(key) === change.present) {
        blockerChanges.delete(key);
      } else if (change.present) {
        next.set(key, change.id);
      } else {
        next.delete(key);
      }
    }
    for (const [key, owner] of expired) {
      if (hydrated.has(`permission:${owner}`) && !cached.has(key)) expired.delete(key);
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
    if (Date.now() < nextSelectionAt) {
      if (effective() !== published) publish("reconcile");
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
  syncSelection();
  const poll = setInterval(syncSelection, ROUTE_POLL_INTERVAL_MS);
  return () => {
    disposed = true;
    generation += 1;
    clearTimeout(retryTimer);
    clearInterval(poll);
    unsubscribe();
    sessions.clear();
    ancestorSyncs.clear();
    executionChanges.clear();
    blockers.clear();
    blockerChanges.clear();
    expired.clear();
  };
}
