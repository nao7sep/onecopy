// Several values named inside a message (interface-styling-conventions, "A
// message names a few values"): the first few, each shortened as identifying
// text, then a count of the rest ("and 12 more"). The count is a control that
// opens the complete list in a RevealPopover, so every value stays reachable
// by keyboard and touch without the message growing with the list.

import { useId, useRef, useState } from "react";
import { useI18n } from "../../i18n/I18nContext";
import IdentifyingText from "./IdentifyingText";
import { LinkButton } from "./Button";
import RevealPopover from "./RevealPopover";

/** How many values a message names before it counts the rest. */
export const NAMED_VALUES = 3;

export default function NameList({
  values,
  path = false,
}: {
  values: readonly string[];
  /** The values are paths: each keeps its final segment. */
  path?: boolean;
}) {
  const { t, list } = useI18n();
  const id = useId();
  const more = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const rest = values.length - NAMED_VALUES;
  const named = values.slice(0, NAMED_VALUES).map((value, index) => (
    // A narrow cap of its own, so one long value cannot take the line.
    <span key={index} className="inline-block max-w-[16rem] align-bottom">
      <IdentifyingText value={value} path={path} inline />
    </span>
  ));
  if (rest <= 0) return <>{list(named)}</>;
  return (
    <>
      {list([
        ...named,
        <LinkButton
          key="more"
          ref={more}
          id={id}
          tone="current"
          aria-haspopup="dialog"
          aria-expanded={open}
          className="underline"
          onClick={() => setOpen((current) => !current)}
        >
          {t("common.countMore", { count: rest })}
        </LinkButton>,
      ])}
      {open && more.current !== null ? (
        <RevealPopover anchor={more.current} labelledBy={id} onClose={() => setOpen(false)}>
          <ul className="space-y-1">
            {values.map((value, index) => (
              <li key={index} className={path ? "break-all" : "break-words"}>{value}</li>
            ))}
          </ul>
        </RevealPopover>
      ) : null}
    </>
  );
}
