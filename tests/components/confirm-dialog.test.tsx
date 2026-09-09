// @vitest-environment happy-dom

import { useState } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import ConfirmDialog from "../../src/components/ConfirmDialog";
import ModalShell from "../../src/components/ModalShell";

afterEach(cleanup);

function confirm(onConfirm = vi.fn(), onCancel = vi.fn()) {
  render(<ConfirmDialog title="Delete item?" message="Delete every copy?"
    confirmLabel="Delete" onConfirm={onConfirm} onCancel={onCancel} />);
  return { onConfirm, onCancel };
}

describe("confirmation command boundary", () => {
  it.each(["{ArrowRight}", "{Tab}"])("focuses Cancel synchronously, then %s Enter confirms", async (step) => {
    const user = userEvent.setup();
    const { onConfirm, onCancel } = confirm();
    const cancel = screen.getByRole("button", { name: "Cancel" });
    expect(document.activeElement).toBe(cancel);
    expect(cancel.className).toContain("focus:ring-2");
    await user.keyboard(step);
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Delete" }));
    expect(onConfirm).not.toHaveBeenCalled();
    await user.keyboard("{Enter}");
    expect(onConfirm).toHaveBeenCalledOnce();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("Left returns to Cancel and arrows at an edge never leave the footer", async () => {
    const user = userEvent.setup();
    const { onConfirm, onCancel } = confirm();
    await user.keyboard("{ArrowLeft}{ArrowRight}{ArrowRight}");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Delete" }));
    await user.keyboard("{ArrowLeft}{Enter}");
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("ignores held activation, composition, and the rejected D shortcut", async () => {
    const user = userEvent.setup();
    const { onConfirm, onCancel } = confirm();
    await user.keyboard("{ArrowRight}");
    const target = document.activeElement!;
    expect(fireEvent.keyDown(target, { key: "Enter", repeat: true })).toBe(false);
    expect(fireEvent.keyDown(target, { key: " ", repeat: true })).toBe(false);
    fireEvent.keyDown(target, { key: "ArrowLeft", isComposing: true });
    fireEvent.keyDown(target, { key: "Escape", isComposing: true });
    await user.keyboard("d");
    expect(document.activeElement).toBe(target);
    expect(onConfirm).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("keeps an initially nested confirmation above its parent and restores parent focus", async () => {
    const user = userEvent.setup();
    const closeParent = vi.fn();
    function Host() {
      const [pending, setPending] = useState(true);
      const [open, setOpen] = useState(true);
      return open && <ModalShell title="Parent" onClose={() => { closeParent(); setOpen(false); }} initialFocus="surface">
        <button>Parent action</button>
        {pending && <ConfirmDialog title="Delete item?" message="Delete every copy?" confirmLabel="Delete"
          onConfirm={vi.fn()} onCancel={() => setPending(false)} />}
      </ModalShell>;
    }
    const opener = document.createElement("button");
    document.body.append(opener);
    opener.focus();
    render(<Host />);
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" }));
    await user.keyboard("{Escape}");
    expect(closeParent).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog", { name: "Delete item?" })).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("dialog", { name: "Parent" }));
    await user.keyboard("{Escape}");
    expect(closeParent).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });

  it("does not enable footer arrows in ordinary modals", () => {
    render(<ModalShell title="Ordinary" onClose={vi.fn()} primaryAction={<button>Apply</button>}>Body</ModalShell>);
    const close = screen.getByText("Close");
    close.focus();
    expect(fireEvent.keyDown(close, { key: "ArrowRight" })).toBe(true);
    expect(document.activeElement).toBe(close);
  });
});
