import { Star } from "lucide-react";
import { faceStarLabel } from "../models/itemPresentation";

export default function FaceRating({ stars }: { stars: 1 | 2 | 3 }) {
  const label = faceStarLabel(stars);
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      className="inline-flex items-center"
    >
      {Array.from({ length: stars }, (_, index) => (
        <Star
          key={index}
          aria-hidden="true"
          className="inline-block h-[1em] w-[1em] fill-current stroke-current"
        />
      ))}
    </span>
  );
}
