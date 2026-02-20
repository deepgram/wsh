import { computed, signal } from "@preact/signals";
import { sessionInfoMap, type ViewMode } from "./sessions";

export interface Group {
  tag: string;
  label: string;
  sessions: string[];
  isSpecial: boolean;
  badgeCount: number;
}

export interface QueueEntry {
  session: string;
  quiescentAt: number;
  status: "pending" | "handled";
}

export const selectedGroups = signal<string[]>(["all"]);

const storedViewModes = JSON.parse(localStorage.getItem("wsh-view-modes") || "{}");
export const viewModePerGroup = signal<Record<string, ViewMode>>(storedViewModes);

export const quiescenceQueues = signal<Record<string, QueueEntry[]>>({});

export type SessionStatus = "running" | "quiescent" | "exited";
export const sessionStatuses = signal<Map<string, SessionStatus>>(new Map());

export const tileLayouts = signal<Record<string, {
  sessions: string[];
  sizes: number[];
}>>(JSON.parse(localStorage.getItem("wsh-tile-layouts") || "{}"));

export const groups = computed<Group[]>(() => {
  const infoMap = sessionInfoMap.value;
  const statuses = sessionStatuses.value;
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

  const allSessions = Array.from(infoMap.keys());
  const allBadge = allSessions.filter(
    (s) => statuses.get(s) === "quiescent" || statuses.get(s) === "exited"
  ).length;
  result.push({
    tag: "all",
    label: "All Sessions",
    sessions: allSessions,
    isSpecial: true,
    badgeCount: allBadge,
  });

  const sortedTags = Array.from(tagGroups.keys()).sort();
  for (const tag of sortedTags) {
    const sessions = tagGroups.get(tag)!;
    const badge = sessions.filter(
      (s) => statuses.get(s) === "quiescent" || statuses.get(s) === "exited"
    ).length;
    result.push({
      tag,
      label: tag,
      sessions,
      isSpecial: false,
      badgeCount: badge,
    });
  }

  if (untagged.length > 0) {
    const badge = untagged.filter(
      (s) => statuses.get(s) === "quiescent" || statuses.get(s) === "exited"
    ).length;
    result.push({
      tag: "untagged",
      label: "Untagged",
      sessions: untagged,
      isSpecial: true,
      badgeCount: badge,
    });
  }

  return result;
});

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
  const queues = { ...quiescenceQueues.value };
  const queue = [...(queues[tag] || [])];
  if (queue.some((e) => e.session === session && e.status === "pending")) return;
  queue.push({ session, quiescentAt: Date.now(), status: "pending" });
  queues[tag] = queue;
  quiescenceQueues.value = queues;
}

export function dismissQueueEntry(tag: string, session: string): void {
  const queues = { ...quiescenceQueues.value };
  const queue = (queues[tag] || []).map((e) =>
    e.session === session ? { ...e, status: "handled" as const } : e
  );
  queues[tag] = queue;
  quiescenceQueues.value = queues;
}
