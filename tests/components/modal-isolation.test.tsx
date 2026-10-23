// @vitest-environment happy-dom
import { useState } from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import ModalShell from "../../src/components/ModalShell";
import { resetModalStack } from "../../src/utils/modalStack";

afterEach(() => { cleanup(); resetModalStack(); });

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
