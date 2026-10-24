// @vitest-environment happy-dom
import { useState } from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import ModalShell from "../../src/components/ModalShell";
import { hasOpenModal, resetModalStack } from "../../src/utils/modalStack";

import { presentEscapedDetail } from "../../src/utils/failureSurface";

afterEach(async () => {
  cleanup();
  document.getElementById("onecopy-escaped-failure")?.remove();
  await waitFor(() => expect(hasOpenModal()).toBe(false));
  resetModalStack();
  vi.restoreAllMocks();
});

function NestedDialogs() {
  const [open, setOpen] = useState(false);
  const [child, setChild] = useState(false);
  return <>
    <button onClick={() => setOpen(true)}>Open parent</button>
    {open && <ModalShell title="Parent" onClose={() => setOpen(false)}>
      <button onClick={() => setChild(true)}>Open child</button>
      {child && <ModalShell title="Child" onClose={() => setChild(false)}>
        <input aria-label="Child field" />
      </ModalShell>}
    </ModalShell>}
  </>;
}

describe("modal background isolation", () => {
  it("isolates nested surfaces and restores focus and interaction one layer at a time", () => {
    render(<NestedDialogs />);
    const opener = screen.getByText("Open parent");
    opener.focus();
    fireEvent.click(opener);
    const childOpener = screen.getByText("Open child");
    expect(opener.closest("[inert]")).not.toBeNull();
    expect(childOpener.closest("[inert]")).toBeNull();
    childOpener.focus();
    fireEvent.click(childOpener);
    const input = screen.getByLabelText("Child field");
    expect(document.activeElement).toBe(input);
    expect(childOpener.closest("[inert]")).not.toBeNull();
    expect(input.closest("[inert]")).toBeNull();
    fireEvent.keyDown(input, { key: "Escape" });
    expect(screen.queryByLabelText("Child field")).toBeNull();
    expect(document.activeElement).toBe(childOpener);
    expect(childOpener.closest("[inert]")).toBeNull();
    expect(opener.closest("[inert]")).not.toBeNull();
    fireEvent.keyDown(childOpener, { key: "Escape" });
    expect(document.activeElement).toBe(opener);
    expect(opener.closest("[inert]")).toBeNull();
  });

  it("isolates newly mounted background siblings and preserves existing inert state", async () => {
    const original = document.createElement("section");
    original.setAttribute("inert", "existing");
    document.body.append(original);
    const view = render(<ModalShell title="Active" onClose={() => undefined}>Content</ModalShell>);
    const late = document.createElement("button");
    document.body.append(late);
    await waitFor(() => expect(late.hasAttribute("inert")).toBe(true));
    view.unmount();
    expect(late.hasAttribute("inert")).toBe(false);
    expect(original.getAttribute("inert")).toBe("existing");
    late.remove(); original.remove();
  });
});


it("keeps recovery interactive above existing and late dialogs through repeated failures", async () => {
  const close = vi.fn();
  const reloadWindow = vi.spyOn(window.location, "reload").mockImplementation(() => {});
  const original = render(<ModalShell title="Settings" onClose={close}><input aria-label="Setting" /></ModalShell>);
  const setting = screen.getByLabelText("Setting");
  expect(document.activeElement).toBe(setting);

  presentEscapedDetail("First failure");
  const recovery = screen.getByRole("alertdialog", { name: "OneCopy needs to reload" });
  const reload = screen.getByRole("button", { name: "Reload window" });
  expect(recovery.hasAttribute("aria-modal")).toBe(true);
  expect(reload.closest("[inert]")).toBeNull();
  expect(setting.closest("[inert]")).not.toBeNull();
  expect(document.activeElement).toBe(reload);
  for (const shiftKey of [false, true]) {
    const tab = new KeyboardEvent("keydown", { key: "Tab", shiftKey, bubbles: true, cancelable: true });
    reload.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(reload);
  }
  fireEvent.keyDown(reload, { key: "Escape" });
  expect(close).not.toHaveBeenCalled();

  render(<ModalShell title="Late result" onClose={close}><input aria-label="Late field" autoFocus /></ModalShell>);
  expect(screen.getByLabelText("Late field").closest("[inert]")).not.toBeNull();
  expect(document.activeElement).toBe(reload);
  original.unmount();
  expect(document.activeElement).toBe(reload);
  presentEscapedDetail("Second failure");
  expect(screen.getAllByRole("alertdialog")).toHaveLength(1);
  expect(recovery.textContent).toContain("Second failure");
  expect(reload.closest("[inert]")).toBeNull();
  fireEvent.click(reload);
  expect(reloadWindow).toHaveBeenCalledOnce();
});
