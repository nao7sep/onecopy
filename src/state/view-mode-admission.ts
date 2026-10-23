export type ViewMode = "fullscreen" | "comparison";

export interface ViewAdmission {
  current(): boolean;
  commit(): boolean;
  abandon(): void;
}

/** Main owns one viewing mode from its first asynchronous preparation through
 * native retirement. A rejected replacement leaves the admitted session live. */
export class ViewModeAdmission {
  private mode: ViewMode | null = null;
  private revision = 0;
  // Each mode owns replacement ordering. Comparison may keep preparing its
  // earlier request while a later selection is only being validated.
  private pending = new Set<number>();
  private active: number | null = null;
  private retiring = new Set<number>();

  owns(mode: ViewMode): boolean { return this.mode === mode; }

  begin(mode: ViewMode): ViewAdmission | null {
    if (this.mode !== null && this.mode !== mode) return null;
    this.mode = mode;
    const revision = ++this.revision;
    this.pending.add(revision);
    return {
      current: () => this.pending.has(revision) || this.active === revision,
      commit: () => {
        if (!this.pending.delete(revision)) return false;
        this.active = revision;
        return true;
      },
      abandon: () => {
        this.pending.delete(revision);
        this.releaseIfIdle();
      },
    };
  }

  /** Invalidate preparation synchronously; release the mode only after the
   * caller's queued native cleanup settles. Failed entry retires only the
   * admitted session, preserving a replacement already being prepared. */
  retire(mode: ViewMode, preservePreparation = false): () => void {
    if (this.mode !== mode) {
      return () => undefined;
    }
    const revision = ++this.revision;
    if (!preservePreparation) this.pending.clear();
    this.active = null;
    this.retiring.add(revision);
    return () => {
      this.retiring.delete(revision);
      this.releaseIfIdle();
    };
  }

  private releaseIfIdle(): void {
    if (this.pending.size === 0 && this.active === null && this.retiring.size === 0) this.mode = null;
  }
}

export const viewModeAdmission = new ViewModeAdmission();
