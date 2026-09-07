import { describe, expect, it, vi } from "vitest";
import {
  createEventInstaller,
  EventInstallation,
} from "../../src/utils/eventInstallation";

describe("EventInstallation", () => {
  it("retains successful listeners after complete installation", async () => {
    const stop = vi.fn();
    const installation = new EventInstallation();

    await installation.add(Promise.resolve(stop));

    expect(stop).not.toHaveBeenCalled();
  });

  it("rolls an installed prefix back in reverse order after a later failure", async () => {
    const calls: string[] = [];
    const installation = new EventInstallation();
    await installation.add(Promise.resolve(() => calls.push("first")));
    await installation.add(Promise.resolve(() => calls.push("second")));

    await expect(
      installation.add(Promise.reject(new Error("third failed"))),
    ).rejects.toThrow("third failed");
    installation.rollback();

    expect(calls).toEqual(["second", "first"]);
  });

  it("continues rollback when one unlisten callback fails", async () => {
    const survivingStop = vi.fn();
    const installation = new EventInstallation();
    await installation.add(Promise.resolve(survivingStop));
    await installation.add(
      Promise.resolve(() => {
        throw new Error("unlisten failed");
      }),
    );

    expect(() => installation.rollback()).not.toThrow();
    expect(survivingStop).toHaveBeenCalledOnce();
  });

  it("shares concurrent attempts and retries after rolling back a failure", async () => {
    const stop = vi.fn();
    const onFailure = vi.fn();
    let attempts = 0;
    const install = createEventInstaller(async (listeners) => {
      attempts += 1;
      await listeners.add(Promise.resolve(stop));
      if (attempts === 1) throw new Error("later listener failed");
    }, onFailure);

    await Promise.all([install(), install()]);
    expect(attempts).toBe(1);
    expect(stop).toHaveBeenCalledOnce();
    expect(onFailure).toHaveBeenCalledOnce();

    await install();
    await install();
    expect(attempts).toBe(2);
    expect(stop).toHaveBeenCalledOnce();
  });
});
