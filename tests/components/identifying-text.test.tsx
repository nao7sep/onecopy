// @vitest-environment happy-dom

// Variable text never sizes its container (interface-styling-conventions,
// "Labels"): identifying text takes one line and is cut with one CSS-drawn
// marker — a name at its end, a path in its middle — and a cut value is
// reachable whole by keyboard and touch, not only by a hover title. A message
// about several values names a few and counts the rest. The layout engine is
// not here, so these specs fake the one measurement the component reads (a
// line's text wider than the line) and check structure and interaction.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import IdentifyingText, { finalSegmentStart } from "../../src/components/ui/IdentifyingText";
import NameList, { NAMED_VALUES } from "../../src/components/ui/NameList";
import ModalShell from "../../src/components/ModalShell";
import OperationResult from "../../src/components/ui/OperationResult";
import { Menu, MenuItem } from "../../src/components/Menu";
import { resetModalStack } from "../../src/utils/modalStack";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  resetModalStack();
});

/** Every line's text is wider than the line, as a long value in a narrow pane. */
function textOverflows() {
  vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(400);
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(120);
}

describe("where a path is cut", () => {
  it("keeps the separator with the final segment", () => {
    expect(finalSegmentStart("/Users/me/Pictures/IMG_0001.jpg")).toBe(18);
    expect(finalSegmentStart("C:\\Photos\\2024\\IMG_0001.jpg")).toBe(14);
    expect(finalSegmentStart("/Volumes/Photos/")).toBe(8);
  });

  it("has nothing to lose without a folder part", () => {
    expect(finalSegmentStart("IMG_0001.jpg")).toBe(-1);
    expect(finalSegmentStart("/Volumes")).toBe(-1);
  });
});

describe("identifying text that fits", () => {
  it("stays plain text on one line, out of the tab order", async () => {
    const user = userEvent.setup();
    render(<IdentifyingText value="IMG_0001.jpg" />);
    const text = screen.getByText("IMG_0001.jpg");
    expect(text.tagName).toBe("SPAN");
    expect(text.className.split(" ")).toEqual(expect.arrayContaining(["block", "truncate", "w-full"]));
    expect(screen.queryByRole("button")).toBeNull();
    await user.tab();
    expect(document.activeElement).toBe(document.body);
  });

  it("renders a fitting path whole, as one run", () => {
    render(<IdentifyingText value="/Volumes/Photos/IMG_0001.jpg" path />);
    expect(screen.getByText("/Volumes/Photos/IMG_0001.jpg").className).toContain("truncate");
  });
});

describe("a cut name", () => {
  it("keeps its beginning and opens whole on Enter, then Escape returns", async () => {
    textOverflows();
    const user = userEvent.setup();
    const name = "A very long holiday photo name from the beach at sunset.jpg";
    render(<IdentifyingText value={name} />);
    const button = screen.getByRole("button", { name });
    // The DOM keeps the whole value; CSS draws the marker at the end.
    expect(button.textContent).toBe(name);
    expect(button.className.split(" ")).toContain("truncate");
    expect(button.getAttribute("aria-haspopup")).toBe("dialog");
    expect(button.getAttribute("aria-expanded")).toBe("false");

    await user.tab();
    expect(document.activeElement).toBe(button);
    await user.keyboard("{Enter}");
    const popover = screen.getByRole("dialog", { name });
    expect(popover.textContent).toBe(name);
    expect(popover.parentElement).toBe(document.body);
    expect(popover.className.split(" ")).toEqual(
      expect.arrayContaining(["max-h-40", "overflow-y-auto", "select-text"]),
    );
    expect(document.activeElement).toBe(popover);
    expect(button.getAttribute("aria-expanded")).toBe("true");

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(button);
  });

  it("opens on a click or tap without reaching the row around it", () => {
    textOverflows();
    const onRow = vi.fn();
    render(
      <div onClick={onRow}>
        <IdentifyingText value="IMG_0001.jpg" />
      </div>,
    );
    fireEvent.click(screen.getByRole("button", { name: "IMG_0001.jpg" }));
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.click(screen.getByRole("dialog"));
    expect(onRow).not.toHaveBeenCalled();
    // A press outside closes it.
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("stays text where the selection's detail already shows it whole", () => {
    textOverflows();
    render(<IdentifyingText value="IMG_0001.jpg" reveal={false} />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText("IMG_0001.jpg").className).toContain("truncate");
  });

  it("closes over a dialog without closing the dialog", async () => {
    textOverflows();
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <ModalShell title="Settings" onClose={onClose}>
        <IdentifyingText value="Built-in Retina Display · 3024×1964" />
      </ModalShell>,
    );
    await user.click(screen.getByRole("button", { name: "Built-in Retina Display · 3024×1964" }));
    expect(screen.getAllByRole("dialog")).toHaveLength(2);
    await user.keyboard("{Escape}");
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe("a cut path", () => {
  it("keeps its final segment whole and loses its middle", async () => {
    textOverflows();
    const user = userEvent.setup();
    const path = "/Users/me/Pictures/Holidays/2024/IMG_0001.jpg";
    render(<IdentifyingText value={path} path />);
    const button = screen.getByRole("button", { name: path });
    const [head, tail] = Array.from(button.children) as HTMLElement[];
    expect(head.textContent).toBe("/Users/me/Pictures/Holidays/2024");
    expect(head.className.split(" ")).toEqual(expect.arrayContaining(["min-w-0", "truncate"]));
    expect(tail.textContent).toBe("/IMG_0001.jpg");
    expect(tail.className.split(" ")).toEqual(expect.arrayContaining(["shrink-0", "max-w-full", "truncate"]));
    // Copying and assistive technology still read it whole.
    expect(button.textContent).toBe(path);

    await user.click(button);
    expect(screen.getByRole("dialog").textContent).toBe(path);
    expect(screen.getByRole("dialog").firstElementChild?.className).toContain("break-all");
  });
});

describe("a message about several values", () => {
  it("names every value when there are only a few", () => {
    render(<p><NameList values={["/a/IMG_1.jpg", "/a/IMG_2.jpg"]} path /></p>);
    expect(screen.getByRole("paragraph").textContent).toBe("/a/IMG_1.jpg and /a/IMG_2.jpg");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("names a few, counts the rest, and opens the complete list", async () => {
    const user = userEvent.setup();
    const paths = Array.from({ length: 15 }, (_, index) => `/dest/IMG_${index + 1}.jpg`);
    render(<p><NameList values={paths} path /></p>);
    const sentence = screen.getByRole("paragraph");
    expect(NAMED_VALUES).toBe(3);
    expect(sentence.textContent).toBe("/dest/IMG_1.jpg, /dest/IMG_2.jpg, /dest/IMG_3.jpg, and 12 more");
    expect(sentence.textContent).not.toContain("IMG_4.jpg");

    await user.click(screen.getByRole("button", { name: "12 more" }));
    const list = screen.getByRole("dialog", { name: "12 more" });
    expect(Array.from(list.querySelectorAll("li"), (item) => item.textContent)).toEqual(paths);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("text to be read", () => {
  it("wraps an operation result whole inside a height limit that scrolls", () => {
    render(<OperationResult level="error">ffmpeg exited with an error</OperationResult>);
    const text = screen.getByText("ffmpeg exited with an error");
    expect(text.className.split(" ")).toEqual(
      expect.arrayContaining(["break-words", "max-h-32", "overflow-y-auto"]),
    );
    expect(text.className).not.toContain("truncate");
  });
});

describe("the shared menu", () => {
  it("keeps every label on one line under a narrow cap, ending past it with an ellipsis", () => {
    render(
      <Menu
        ariaLabel="Test menu"
        trigger={(props) => <button {...props} aria-label="Open test menu">Menu</button>}
      >
        <MenuItem onSelect={() => undefined}>Volver a ejecutar el asistente de configuración</MenuItem>
      </Menu>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Open test menu" }));
    const menu = screen.getByRole("menu");
    expect(menu.className).toContain("max-w-[min(24rem,calc(100vw-1rem))]");
    expect(menu.className).not.toContain("overflow-auto");
    const item = screen.getByRole("menuitem");
    expect(item.className).toContain("whitespace-nowrap");
    const label = item.firstElementChild as HTMLElement;
    expect(label.textContent).toBe("Volver a ejecutar el asistente de configuración");
    expect(label.className.split(" ")).toEqual(expect.arrayContaining(["min-w-0", "truncate"]));
  });
});
