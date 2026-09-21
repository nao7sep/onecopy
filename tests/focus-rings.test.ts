// @vitest-environment node
//
// The ring belongs to what the key acts on (composite-control-conventions). A
// container that holds focus on its items' behalf — a list with an active row, a
// scroll region, a modal body — draws no ring of its own, because
// `:focus-visible` is no guard for it: any key pressed after a click turns it on,
// and since Safari 27 so does focus a script moves after keyboard use. The transcript
// panel, the activity trace and the viewer windows each drew their own ring, and
// the comparison lists and the text surface fell back to the browser's.
//
// Every place this app makes something focusable is found here by its text, not
// from memory, and each has to be one of three things: a control or item, which
// may ring; a container the covering rule in App.css reaches; or a site listed
// below with the reason it is neither. A new one that is none of these fails.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

const ROOT = join(__dirname, "..");
const SOURCE_DIR = "src";
const COVERING_STYLESHEET = "src/App.css";

// Focus sites the text alone cannot place, and why each is not a ringing container.
// Each entry covers only the one kind of site it explains, so anything else added
// to the same file is still placed on its own merits.
const EXPLAINED: Record<string, { kind: Site["kind"]; reason: string }> = {
  "src/utils/focusTrap.ts": {
    kind: "attribute",
    reason: "a selector that names focusable elements, not an element",
  },
};

// A composite that owns its items, or a region that scrolls on their behalf.
const CONTAINER_ROLES = [
  "listbox", "tree", "grid", "treegrid", "region", "tabpanel",
  "dialog", "alertdialog", "document", "log",
  "radiogroup", "tablist", "menu", "menubar", "toolbar",
];
const ITEM_ROLES = [
  "option", "tab", "treeitem", "row", "gridcell", "menuitem", "menuitemradio",
  "menuitemcheckbox", "radio", "checkbox", "switch", "button", "link",
  "separator", "slider", "spinbutton", "textbox", "combobox",
];
const CONTROL_TAGS = ["button", "a", "input", "select", "textarea", "summary"];

const FOCUS_SITE =
  /tabIndex\s*=\s*[{"']|tabindex\s*=\s*["'{]|tabIndex\s*:|\.tabIndex\s*=|setAttribute\(\s*["']tabindex/g;
// A hidden outline, in the three places a site can say so: Tailwind's two
// spellings (`outline-none`, and v4's `outline-hidden`, which keeps a transparent
// one for forced-colors mode) and an inline style.
const OUTLINE_OFF =
  /(^|[\s"'`])(focus:|focus-visible:)?outline-(none|hidden)\b|outline\s*:\s*["']?none\b/;
const CONTAINER_RING =
  /(^|[\s"'`])(focus:|focus-visible:)(ring\b|ring-(?!0\b)|outline-(?!(none|hidden|0)\b))/;

type Site = { file: string; line: number; kind: "attribute" | "object" | "dom"; context: string };

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sourceFiles(path);
    return /\.(tsx?|jsx?|html)$/.test(name) && !/\.(test|spec)\./.test(name) ? [path] : [];
  });
}

// From a match inside a JSX or HTML opening tag, the whole tag; from an object key,
// the whole object. Braces are counted so an arrow function's `=>` or a nested
// object does not end either one early.
function enclosing(text: string, at: number, open: string, close: string): string {
  let depth = 0;
  let start = at;
  for (; start > 0; start--) {
    const c = text.charAt(start);
    if (c === "}") depth++;
    else if (c === "{") {
      if (depth === 0 && open === "{") break;
      depth--;
    } else if (open === "<" && depth === 0 && c === "<" && /[A-Za-z]/.test(text.charAt(start + 1))) break;
  }
  depth = 0;
  let end = at;
  for (; end < text.length; end++) {
    const c = text.charAt(end);
    if (c === "{") depth++;
    else if (c === "}") {
      if (depth === 0 && close === "}") break;
      depth--;
    } else if (close === ">" && depth === 0 && c === ">" && text.charAt(end - 1) !== "=") break;
  }
  return text.slice(start, end + 1);
}

// Comments are blanked, newlines kept, so prose that mentions `tabIndex={-1}` is
// not a site and every real one keeps its line number. Only where a comment can
// only be a comment: a whole-line `//`, blanked first, then a block that opens at
// the start of a line or as JSX's `{/*`. A `/*` anywhere else — inside a `//`
// line's prose, or a string — would otherwise open a comment that swallows real
// code up to the next `*/`, and every site in it with it.
function withoutComments(text: string): string {
  const blank = (s: string) => s.replace(/[^\n]/g, " ");
  return text
    .replace(/^[ \t]*\/\/.*$/gm, blank)
    .replace(/(^[ \t]*|\{)(\/\*[\s\S]*?\*\/)/gm, (_all, lead: string, body: string) => lead + blank(body));
}

function focusSites(): Site[] {
  return sourceFiles(join(ROOT, SOURCE_DIR)).flatMap((path) => {
    const text = withoutComments(readFileSync(path, "utf8"));
    const file = relative(ROOT, path);
    return [...text.matchAll(FOCUS_SITE)].map((match) => {
      const at = match.index ?? 0;
      const token = match[0];
      const line = text.slice(0, at).split("\n").length;
      if (token.startsWith(".") || token.startsWith("setAttribute")) {
        return { file, line, kind: "dom" as const, context: "" };
      }
      if (token.startsWith("tabIndex") && token.includes(":")) {
        return { file, line, kind: "object" as const, context: enclosing(text, at, "{", "}") };
      }
      return { file, line, kind: "attribute" as const, context: enclosing(text, at, "<", ">") };
    });
  });
}

function roleOf(context: string): string | undefined {
  return context.match(/role\s*[=:]\s*\{?\s*["']([\w-]+)["']/)?.[1];
}

// The container's own class names, so a stylesheet rule that rings it by class —
// not by role — is caught as well.
function classesOf(context: string): string[] {
  const value =
    context.match(/className\s*=\s*\{?\s*[`"']([^`"']*)/)?.[1] ??
    context.match(/class\s*=\s*"([^"]*)"/)?.[1] ??
    "";
  return value.split(/\s+/).filter((name) => /^[A-Za-z_][\w-]*$/.test(name));
}

function placement(site: Site): "control" | "covered" | "suppressed" | "explained" | undefined {
  if (site.kind !== "dom") {
    const tag = site.context.match(/^<([A-Za-z][\w.]*)/)?.[1];
    const role = roleOf(site.context);
    if (tag && CONTROL_TAGS.includes(tag)) return "control";
    if (role && ITEM_ROLES.includes(role)) return "control";
    if (role && CONTAINER_ROLES.includes(role)) return "covered";
    if (site.context.includes("data-passive-scroll-region")) return "covered";
    if (OUTLINE_OFF.test(site.context)) return "suppressed";
    if (suppressedByClass(site.context)) return "suppressed";
  }
  return EXPLAINED[site.file]?.kind === site.kind ? "explained" : undefined;
}

type Rule = { selector: string; body: string };

function rules(css: string): Rule[] {
  const bare = css.replace(/\/\*[\s\S]*?\*\//g, "");
  return [...bare.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => ({
    selector: (m[1] ?? "").trim().replace(/\s+/g, " "),
    body: m[2] ?? "",
  }));
}

// The element each selector in a list actually styles: its last compound, after
// any combinator. Focus on a container may still paint its active row —
// `[role="listbox"]:focus-visible .row` styles the row — so only a selector whose
// subject is itself a focused container counts against it.
function subjects(selector: string): string[] {
  const list: string[] = [];
  let depth = 0;
  let current = "";
  for (const c of selector) {
    if (c === "(" || c === "[") depth++;
    else if (c === ")" || c === "]") depth--;
    if (c === "," && depth === 0) {
      list.push(current);
      current = "";
    } else current += c;
  }
  list.push(current);
  return list.map((part) => {
    const s = part.trim();
    let level = 0;
    let last = 0;
    for (let i = 0; i < s.length; i++) {
      const c = s.charAt(i);
      if (c === "(" || c === "[") level++;
      else if (c === ")" || c === "]") level--;
      else if (level === 0 && /[\s>+~]/.test(c)) last = i + 1;
    }
    return s.slice(last).trim();
  });
}

// Whether a declaration block paints something around its element: an outline
// that is not switched off, or a box shadow, which is how ring utilities draw.
function draws(body: string): boolean {
  return body.split(";").some((declaration) => {
    const [property, ...rest] = declaration.split(":");
    const name = property?.trim().toLowerCase() ?? "";
    const value = rest.join(":").trim().toLowerCase();
    if (!value) return false;
    if (name === "outline" || name === "outline-style") return !/^(none|0)\b/.test(value);
    if (name === "outline-width") return !/^0(px)?$/.test(value);
    if (name === "box-shadow") return value !== "none";
    return false;
  });
}

// A page written out as a string — a startup or message window the main process
// builds — never loads the app's stylesheet. Its own <style>, if it has one, is
// what reaches its containers.
function ownStyle(file: string): string | undefined {
  const text = readFileSync(join(ROOT, file), "utf8");
  const blocks = [...text.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)].map((m) => m[1] ?? "");
  return blocks.length ? blocks.join("\n") : undefined;
}

function stylesheetFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return stylesheetFiles(path);
    return name.endsWith(".css") ? [path] : [];
  });
}

const stylesheet = readFileSync(join(ROOT, COVERING_STYLESHEET), "utf8");
const sites = focusSites();

// Every stylesheet the app has, and every page that carries its own: a ring can
// come from any of them, and so can the rule that turns one off.
const everyRule = [
  ...stylesheetFiles(join(ROOT, SOURCE_DIR)).map((path) => readFileSync(path, "utf8")),
  ...new Set(sites.map((site) => ownStyle(site.file)).filter((css) => css !== undefined)),
].flatMap((css) => rules(css));

// A plain-CSS app turns an outline off by class, in a stylesheet, rather than
// with a utility on the element: a focus rule whose subject is one of the site's
// own classes and whose outline is none.
function suppressedByClass(context: string): boolean {
  const classes = classesOf(context);
  return everyRule.some(
    (rule) =>
      /outline:\s*none/.test(rule.body) &&
      subjects(rule.selector).some(
        (subject) =>
          /:focus(-visible)?(?![\w-])/.test(subject) &&
          classes.some((name) => new RegExp(`\\.${name}(?![\\w-])`).test(subject)),
      ),
  );
}

describe("focus rings belong to what the key acts on", () => {
  it("finds this app's focus sites at all", () => {
    // A pattern that silently stopped matching would pass every check below.
    expect(sites.length).toBeGreaterThan(3);
  });

  it("places every focus site as a control, a covered container, or an explained exception", () => {
    const unplaced = sites
      .filter((site) => placement(site) === undefined)
      .map((site) => `${site.file}:${site.line} ${site.context.slice(0, 120)}`);
    expect(unplaced).toEqual([]);
  });

  it("gives no container a ring utility of its own", () => {
    const ringed = sites
      .filter((site) => ["covered", "suppressed"].includes(placement(site) ?? ""))
      .filter((site) => CONTAINER_RING.test(site.context))
      .map((site) => `${site.file}:${site.line}`);
    expect(ringed).toEqual([]);
  });

  it("covers the containers of a page that carries its own stylesheet, in that stylesheet", () => {
    const uncovered = sites
      .filter((site) => placement(site) === "covered")
      .filter((site) => ownStyle(site.file) !== undefined)
      .filter((site) => {
        const role = roleOf(site.context);
        const key = role ? `[role="${role}"]` : "[data-passive-scroll-region]";
        return !rules(ownStyle(site.file)!).some(
          (rule) =>
            rule.selector.includes(key) &&
            rule.selector.includes(":focus-visible") &&
            /outline:\s*none/.test(rule.body),
        );
      })
      .map((site) => `${site.file}:${site.line}`);
    expect(uncovered).toEqual([]);
  });

  it("turns the outline off for every container role in one unlayered rule", () => {
    const covering = rules(stylesheet).find(
      (rule) =>
        rule.selector.startsWith(":is(") &&
        rule.selector.endsWith(":focus-visible") &&
        /outline:\s*none/.test(rule.body),
    );
    expect(covering, "the covering rule is missing").toBeDefined();
    for (const role of CONTAINER_ROLES) {
      expect(covering!.selector).toContain(`[role="${role}"]`);
    }
    expect(covering!.selector).toContain("[data-passive-scroll-region]");
  });

  it("lets no other focus rule draw on a container, by role or by class", () => {
    const containerClasses = [
      ...new Set(
        sites
          .filter((site) => ["covered", "suppressed"].includes(placement(site) ?? ""))
          .flatMap((site) => classesOf(site.context)),
      ),
    ];
    const byClass = containerClasses.length
      ? `|\\.(${containerClasses.join("|")})(?![\\w-])`
      : "";
    const containerSelector = new RegExp(
      `\\[role="(${CONTAINER_ROLES.join("|")})"\\]|\\[data-passive-scroll-region\\]${byClass}`,
    );
    const drawing = everyRule
      .filter((rule) => draws(rule.body))
      .filter((rule) =>
        subjects(rule.selector).some(
          (subject) =>
            /:focus(-visible)?(?![\w-])/.test(subject) && containerSelector.test(subject),
        ),
      )
      .map((rule) => rule.selector);
    expect(drawing).toEqual([]);
  });
});
