export interface ReadingPosition { key: string; offset: number }

export function captureReadingPosition(viewport: HTMLElement | null): ReadingPosition | null {
  if (viewport === null || viewport.scrollTop <= 0) return null;
  const top = viewport.getBoundingClientRect().top;
  const row = [...viewport.querySelectorAll<HTMLElement>("[data-activity-anchor]")]
    .find((element) => element.getBoundingClientRect().bottom > top);
  return row ? { key: row.dataset.activityAnchor!, offset: row.getBoundingClientRect().top - top } : null;
}

export function restoreReadingPosition(viewport: HTMLElement | null, position: ReadingPosition | null): void {
  if (viewport === null || position === null) return;
  const row = [...viewport.querySelectorAll<HTMLElement>("[data-activity-anchor]")]
    .find((element) => element.dataset.activityAnchor === position.key);
  if (row) viewport.scrollTop += row.getBoundingClientRect().top - viewport.getBoundingClientRect().top - position.offset;
}
