export function passiveScrollKey(
  key: string,
  shiftKey: boolean,
  viewportHeight: number,
): number | "start" | "end" | null {
  switch (key) {
    case "ArrowUp":
      return -40;
    case "ArrowDown":
      return 40;
    case "PageUp":
      return -Math.max(40, Math.floor(viewportHeight * 0.9));
    case "PageDown":
      return Math.max(40, Math.floor(viewportHeight * 0.9));
    case "Home":
      return "start";
    case "End":
      return "end";
    case " ":
      return (shiftKey ? -1 : 1) * Math.max(40, Math.floor(viewportHeight * 0.9));
    default:
      return null;
  }
}
