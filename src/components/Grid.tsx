import { extLabel, thumbUrl, type SectionItem } from "../models/items";

// The center-pane thumbnail grid. Native lazy loading keeps a large month
// cheap; every pixel comes from the mediacache protocol, never original files.

function Tile({ item }: { item: SectionItem }) {
  return (
    <figure className="relative w-40">
      <div className="flex h-32 w-40 items-center justify-center overflow-hidden rounded border border-border bg-surface">
        {item.hash !== null && item.hasThumb ? (
          <img
            src={thumbUrl(item.hash)}
            alt={item.fileName}
            loading="lazy"
            className="max-h-full max-w-full object-contain"
          />
        ) : (
          <span className="text-lg font-semibold text-ink-muted">
            {extLabel(item.fileName)}
          </span>
        )}
      </div>
      {item.copyCount > 1 ? (
        <span className="absolute right-1 top-1 rounded bg-primary-surface px-1 text-xs text-primary">
          ×{item.copyCount}
        </span>
      ) : null}
      <figcaption
        className="mt-0.5 w-40 truncate text-xs text-ink-muted"
        title={item.fileName}
      >
        {item.fileName}
      </figcaption>
    </figure>
  );
}

export default function Grid({
  items,
  loading,
}: {
  items: SectionItem[];
  loading: boolean;
}) {
  if (loading) {
    return <p className="text-ink-muted">Loading…</p>;
  }
  if (items.length === 0) {
    return <p className="text-ink-muted">Nothing in this section</p>;
  }
  return (
    <div className="flex flex-wrap content-start gap-3 overflow-y-auto p-3">
      {items.map((item) => (
        <Tile key={item.hash ?? `path-${item.pathId}`} item={item} />
      ))}
    </div>
  );
}
