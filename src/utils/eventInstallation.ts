import {
  listen,
  type EventCallback,
  type UnlistenFn,
} from "@tauri-apps/api/event";

/**
 * Owns listener registration until an app-lifetime installation is complete.
 * A later registration failure must remove the already-installed prefix before
 * the feature can report failure or retry.
 */
export class EventInstallation {
  private installed: UnlistenFn[] = [];

  retain(unlisten: UnlistenFn): void {
    this.installed.push(unlisten);
  }

  async add(registration: Promise<UnlistenFn>): Promise<void> {
    this.retain(await registration);
  }

  async listen<T>(event: string, handler: EventCallback<T>): Promise<void> {
    await this.add(listen<T>(event, handler));
  }

  rollback(): void {
    for (const unlisten of this.installed.splice(0).reverse()) {
      try {
        unlisten();
      } catch {
        // A failed unlisten cannot make the remaining rollback conditional.
      }
    }
  }
}

/**
 * Creates one app-lifetime installer. Concurrent callers share an attempt;
 * a failed attempt rolls back its installed prefix and may be retried later.
 * The returned promise resolves after a contained failure so one optional
 * feature cannot abort the rest of application bootstrap.
 */
export function createEventInstaller(
  register: (listeners: EventInstallation) => Promise<void>,
  onFailure: (error: unknown) => void,
): () => Promise<void> {
  let installation: Promise<void> | null = null;

  const install = async (): Promise<void> => {
    const listeners = new EventInstallation();
    try {
      await register(listeners);
    } catch (error) {
      listeners.rollback();
      onFailure(error);
      throw error;
    }
  };

  return () => {
    installation ??= install();
    const attempt = installation;
    return attempt.catch(() => {
      if (installation === attempt) installation = null;
    });
  };
}
