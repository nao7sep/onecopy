// The shortcuts help surface's content, as DATA rather than markup.
//
// A hand-maintained help list drifts silently: nothing connects a printed
// chord to a live binding, so a row can outlive the key it describes and read
// as a bug in the app rather than in the list. Holding the rows here means the
// suite can walk them, and it is what makes "does this key actually work?" a
// question with an answer.
//
// Chords follow the keyboard-shortcut-conventions: spelled-out key names, `+`
// between a modifier and a key, `/` between alternatives that share one, and
// the RUNNING platform's modifier word (both Cmd and Ctrl always fire).

import { primaryModWord } from "../utils/shortcuts";

export interface ShortcutRow {
  chord: string;
  action: string;
}

export interface ShortcutGroup {
  title: string;
  /** Where these chords fire — every group is scoped to a focused surface,
   * and the unstated scope was the one gap in an otherwise-pinned catalogue:
   * a chord pressed with the wrong surface focused looks broken, not scoped. */
  context: string;
  rows: ShortcutRow[];
}

export function shortcutGroups(): ShortcutGroup[] {
  const mod = primaryModWord();
  return [
    {
      title: "Browsing",
      context: "when the photo grid has focus",
      rows: [
        { chord: "Arrows", action: "Move the selection" },
        { chord: "Home / End", action: "First or last item" },
        { chord: "Page Up / Page Down", action: "Move by a screenful" },
        { chord: "Shift+Arrows", action: "Extend the selection" },
        { chord: "Click", action: "Select or deselect one item" },
        { chord: "Shift+Click", action: "Select a range" },
      ],
    },
    {
      title: "Looking",
      context: "anywhere in the main window",
      rows: [
        { chord: "Space", action: "Open Quick View (play/pause when its video has focus)" },
        { chord: "Enter", action: "Compare similar photos (≈ badge)" },
        { chord: "F", action: "Fullscreen (in the preview window)" },
        { chord: "Escape", action: "Close Quick View, leave fullscreen, or close the preview window" },
      ],
    },
    {
      title: "Culling",
      context: "when the photo grid has focus",
      rows: [
        { chord: "Delete / Backspace", action: "Trash the item and every copy" },
        { chord: "Shift+Delete", action: "Delete permanently, after confirming" },
      ],
    },
    {
      title: "Comparison view",
      context: "while a comparison is open",
      rows: [
        { chord: "1–9 / 0 / A–F", action: "Mark the photo in that slot as a keeper" },
        { chord: "Shift+1–9/0/A–F", action: "Not similar — remove that slot from the group (never deletes)" },
        { chord: "Left / Right", action: "Page through the group (marks persist; nothing deletes)" },
        { chord: "S", action: "Shortlist — see the marked photos side by side" },
        { chord: "Enter", action: "Next unseen page; once all pages are seen, commit — keep the marked, trash the rest" },
        { chord: "Shift+Enter", action: "Commit, deleting the rest permanently" },
        { chord: "Click", action: "Toggle the photo in that slot as a keeper" },
        { chord: "Escape", action: "Leave — nothing is deleted, marks are discarded" },
      ],
    },
    {
      title: "Destinations tree",
      context: "when the destinations tree has focus",
      rows: [
        { chord: "Enter", action: "Move the selection here, trash the other copies" },
        { chord: "Shift+Enter", action: "Move here, permanently delete the rest" },
        { chord: `${mod}+Enter`, action: "Copy here, leave everything in place" },
      ],
    },
    {
      title: "App",
      context: "anywhere",
      rows: [
        { chord: `${mod}+R`, action: "Recheck this section" },
        { chord: `${mod}+Comma`, action: "Settings" },
        { chord: `${mod}+Slash / Question`, action: "This help" },
        { chord: `${mod}+Equal/Plus/Semicolon`, action: "Zoom in" },
        { chord: `${mod}+Minus`, action: "Zoom out" },
        { chord: `${mod}+0`, action: "Reset zoom" },
      ],
    },
  ];
}
