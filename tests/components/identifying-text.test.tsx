// @vitest-environment happy-dom

// Variable text never sizes its container (interface-styling-conventions,
// "Labels"): identifying text takes one line and is cut with one CSS-drawn
// marker — a name at its end, a path in its middle, keeping the final
// segment — and the DOM keeps the whole value. The layout engine is not
// here, so these specs check structure, not pixels.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { displayNativePath } from "../../src/utils/nativePath";
import IdentifyingText, { finalSegmentStart } from "../../src/components/ui/IdentifyingText";
import OperationResult from "../../src/components/ui/OperationResult";
import { Menu, MenuItem } from "../../src/components/Menu";

afterEach(cleanup);

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

describe("identifying text", () => {
  it("sets a name on one line that keeps its beginning, with the whole as its tooltip", () => {
    const name = "A very long holiday photo name from the beach at sunset.jpg";
    render(<IdentifyingText value={name} />);
    const text = screen.getByText(name);
    expect(text.className.split(" ")).toEqual(expect.arrayContaining(["block", "min-w-0", "truncate"]));
    expect(text.title).toBe(name);
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("sets a path as folders that give way first and a final segment kept whole", () => {
    const path = "/Users/me/Pictures/Holidays/2024/IMG_0001.jpg";
    const { container } = render(<IdentifyingText value={path} path />);
    const line = container.firstElementChild as HTMLElement;
    expect(line.textContent).toBe(path);
    expect(line.title).toBe(path);
    const [head, tail] = Array.from(line.children) as HTMLElement[];
    expect(head.textContent).toBe("/Users/me/Pictures/Holidays/2024");
    expect(head.className.split(" ")).toEqual(expect.arrayContaining(["min-w-0", "truncate"]));
    expect(tail.textContent).toBe("/IMG_0001.jpg");
    expect(tail.className.split(" ")).toEqual(expect.arrayContaining(["shrink-0", "max-w-full", "truncate"]));
  });

  it("lets a site name its own tooltip", () => {
    const { container } = render(<IdentifyingText value="/a · /b" path title={"/a\n/b"} />);
    expect((container.firstElementChild as HTMLElement).title).toBe("/a\n/b");
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


describe("native path labels", () => {
  it("renders invalid bytes and UTF-16 units visibly, preserving the path split", () => {
    const { container } = render(<IdentifyingText value={"C:\\folder\u0000wd800\\photo\u0000bff.jpg"} path />);
    const line = container.firstElementChild as HTMLElement;
    expect(line.textContent).toBe("C:\\folder\\uD800\\photo\\xFF.jpg");
    expect(line.title).toBe(line.textContent);
    expect(line.children[0].textContent).toBe("C:\\folder\\uD800");
    expect(line.children[1].textContent).toBe("\\photo\\xFF.jpg");
  });

  it("keeps distinct native names and the genuine replacement character distinct", () => {
    const names = ["photo\u0000bfe.jpg", "photo\u0000bff.jpg", "photo\ufffd.jpg"];
    const { container } = render(<>{names.map((name) => <IdentifyingText key={name} value={name} />)}</>);
    expect(Array.from(container.children, (line) => line.textContent)).toEqual([
      "photo\\xFE.jpg", "photo\\xFF.jpg", "photo\ufffd.jpg",
    ]);
  });
});


it("preserves valid Unicode and literal escape-looking filenames", () => {
  const path = "C:\\Photos\\日本語-\ud83d\udcf7-\ufffd-\\xFF-\\uD800.jpg";
  expect(displayNativePath(path)).toBe(path);
});
