import { computed, signal } from "@preact/signals";
import { sessionInfoMap, type ViewMode } from "./sessions";

export type GroupByMode = "tag" | "server";
export const groupBy = signal<GroupByMode>(
  (localStorage.getItem("wsh-group-by") as GroupByMode) || "tag"
);

export function setGroupBy(mode: GroupByMode): void {
  groupBy.value = mode;
  localStorage.setItem("wsh-group-by", mode);
}

export interface Group {
  tag: string;
  label: string;
  sessions: string[];
  isSpecial: boolean;
}

export interface QueueEntry {
  session: string;
  idleAt: number;
  status: "pending" | "acknowledged";
}

export const selectedGroups = signal<string[]>(["all"]);

const storedCollapsed: string[] = JSON.parse(localStorage.getItem("wsh-collapsed-groups") || "[]");
export const collapsedGroups = signal<Set<string>>(new Set(storedCollapsed));

export function toggleGroupCollapsed(tag: string): void {
  const updated = new Set(collapsedGroups.value);
  if (updated.has(tag)) {
    updated.delete(tag);
  } else {
    updated.add(tag);
  }
  collapsedGroups.value = updated;
  localStorage.setItem("wsh-collapsed-groups", JSON.stringify(Array.from(updated)));
}

const storedViewModes = JSON.parse(localStorage.getItem("wsh-view-modes") || "{}");
export const viewModePerGroup = signal<Record<string, ViewMode>>(storedViewModes);

export const idleQueues = signal<Record<string, QueueEntry[]>>({});

export type SessionStatus = "running" | "idle";
export const sessionStatuses = signal<Map<string, SessionStatus>>(new Map());

export const tileLayouts = signal<Record<string, {
  sessions: string[];
  sizes: number[];
}>>(JSON.parse(localStorage.getItem("wsh-tile-layouts") || "{}"));

export const groups = computed<Group[]>(() => {
  const infoMap = sessionInfoMap.value;
  const mode = groupBy.value;

  if (mode === "server") {
    return computeServerGroups(infoMap);
  }
  return computeTagGroups(infoMap);
});

function computeTagGroups(infoMap: Map<string, import("../api/types").SessionInfo>): Group[] {
  const tagGroups = new Map<string, string[]>();
  const untagged: string[] = [];

  for (const [name, info] of infoMap) {
    if (info.tags.length === 0) {
      untagged.push(name);
    } else {
      for (const tag of info.tags) {
        const group = tagGroups.get(tag) || [];
        group.push(name);
        tagGroups.set(tag, group);
      }
    }
  }

  const result: Group[] = [];

  const sortedTags = Array.from(tagGroups.keys()).sort();
  for (const tag of sortedTags) {
    const sessions = tagGroups.get(tag)!;
    result.push({
      tag,
      label: tag,
      sessions,
      isSpecial: false,
    });
  }

  if (untagged.length > 0) {
    result.push({
      tag: "untagged",
      label: "Untagged",
      sessions: untagged,
      isSpecial: true,
    });
  }

  const allSessions = Array.from(infoMap.keys());
  result.push({
    tag: "all",
    label: "All Sessions",
    sessions: allSessions,
    isSpecial: true,
  });

  return result;
}

function computeServerGroups(infoMap: Map<string, import("../api/types").SessionInfo>): Group[] {
  const serverGroups = new Map<string, string[]>();
  const local: string[] = [];

  for (const [name, info] of infoMap) {
    const server = info.server;
    if (!server) {
      local.push(name);
    } else {
      const group = serverGroups.get(server) || [];
      group.push(name);
      serverGroups.set(server, group);
    }
  }

  const result: Group[] = [];

  // Local sessions first
  if (local.length > 0) {
    result.push({
      tag: "local",
      label: "Local",
      sessions: local,
      isSpecial: true,
    });
  }

  const sortedServers = Array.from(serverGroups.keys()).sort();
  for (const server of sortedServers) {
    const sessions = serverGroups.get(server)!;
    result.push({
      tag: `server:${server}`,
      label: server,
      sessions,
      isSpecial: false,
    });
  }

  const allSessions = Array.from(infoMap.keys());
  result.push({
    tag: "all",
    label: "All Sessions",
    sessions: allSessions,
    isSpecial: true,
  });

  return result;
}

export const activeGroupSessions = computed<string[]>(() => {
  const selected = selectedGroups.value;
  const allGroups = groups.value;
  const sessionSet = new Set<string>();
  for (const tag of selected) {
    const group = allGroups.find((g) => g.tag === tag);
    if (group) {
      for (const s of group.sessions) sessionSet.add(s);
    }
  }
  return Array.from(sessionSet);
});

export function getViewModeForGroup(tag: string): ViewMode {
  return viewModePerGroup.value[tag] || "carousel";
}

export function setViewModeForGroup(tag: string, mode: ViewMode): void {
  const updated = { ...viewModePerGroup.value, [tag]: mode };
  viewModePerGroup.value = updated;
  localStorage.setItem("wsh-view-modes", JSON.stringify(updated));
}

export function enqueueSession(tag: string, session: string): void {
  const queues = { ...idleQueues.value };
  const queue = [...(queues[tag] || [])];
  const idx = queue.findIndex((e) => e.session === session);
  if (idx !== -1) {
    // Re-activate existing entry as pending with fresh timestamp
    queue[idx] = { ...queue[idx], status: "pending", idleAt: Date.now() };
  } else {
    queue.push({ session, idleAt: Date.now(), status: "pending" });
  }
  queues[tag] = queue;
  idleQueues.value = queues;
}

export function dismissQueueEntry(tag: string, session: string): void {
  const queues = { ...idleQueues.value };
  const queue = (queues[tag] || []).map((e) =>
    e.session === session && e.status === "pending"
      ? { ...e, status: "acknowledged" as const }
      : e
  );
  queues[tag] = queue;
  idleQueues.value = queues;
}

export function removeQueueEntry(tag: string, session: string): void {
  const queues = { ...idleQueues.value };
  const queue = (queues[tag] || []).filter((e) => e.session !== session);
  queues[tag] = queue;
  idleQueues.value = queues;
}

export function getGroupStatusCounts(group: Group): { running: number; idle: number } {
  const statuses = sessionStatuses.value;
  let idle = 0;
  let running = 0;
  for (const s of group.sessions) {
    if (statuses.get(s) === "idle") {
      idle++;
    } else {
      running++;
    }
  }
  return { running, idle };
}
