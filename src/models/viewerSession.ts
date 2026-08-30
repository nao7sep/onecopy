export type ViewerPresentation = "quick" | "fullscreen";
export type ViewerSequenceScope = "section" | "selection";

export interface ViewerMember {
  key: string;
  pathId: number;
}

export interface ViewerSession {
  presentation: ViewerPresentation;
  members: ViewerMember[];
  index: number;
  scope: ViewerSequenceScope;
}

/** Freezes the transient viewer's membership and order at entry. */
export function createViewerSession(
  presentation: ViewerPresentation,
  displayedMembers: ViewerMember[],
  selectedKeys: Set<string>,
  anchor: string,
): ViewerSession | null {
  if (!selectedKeys.has(anchor) || !displayedMembers.some((member) => member.key === anchor)) {
    return null;
  }
  const scope: ViewerSequenceScope = selectedKeys.size === 1 ? "section" : "selection";
  const members =
    scope === "section"
      ? [...displayedMembers]
      : displayedMembers.filter((member) => selectedKeys.has(member.key));
  const index = members.findIndex((member) => member.key === anchor);
  return index < 0 ? null : { presentation, members, index, scope };
}

export function viewerCurrentKey(session: ViewerSession | null): string | null {
  return session?.members[session.index]?.key ?? null;
}

export type ViewerMove = "previous" | "next" | "first" | "last";

/** Viewer navigation is deliberately clamped: it never wraps. */
export function moveViewerSession(session: ViewerSession, move: ViewerMove): ViewerSession {
  const last = session.members.length - 1;
  const index =
    move === "previous"
      ? Math.max(0, session.index - 1)
      : move === "next"
        ? Math.min(last, session.index + 1)
        : move === "first"
          ? 0
          : last;
  return index === session.index ? session : { ...session, index };
}

/** Removes vanished members while retaining the current member whenever it
 * survives. If it vanished, the item that followed it wins, then the one
 * before it; this falls out of retaining the old ordinal in the filtered
 * sequence. */
export function reconcileViewerSession(
  session: ViewerSession,
  liveMembers: ViewerMember[],
): ViewerSession | null {
  const current = viewerCurrentKey(session);
  const byKey = new Map(liveMembers.map((member) => [member.key, member]));
  const byPath = new Map(liveMembers.map((member) => [member.pathId, member]));
  const members = session.members.flatMap((member) => {
    const live = byKey.get(member.key) ?? byPath.get(member.pathId);
    return live === undefined ? [] : [live];
  });
  if (members.length === 0) return null;
  const survivingIndex =
    current === null
      ? -1
      : members.findIndex(
          (member) => member.key === current || member.pathId === session.members[session.index]?.pathId,
        );
  const index =
    survivingIndex >= 0 ? survivingIndex : Math.min(session.index, members.length - 1);
  const unchanged =
    index === session.index &&
    members.length === session.members.length &&
    members.every((member, position) => member.key === session.members[position].key);
  return unchanged ? session : { ...session, members, index };
}
