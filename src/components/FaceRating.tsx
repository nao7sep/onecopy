import { Star } from "lucide-react";
import { useI18n } from "../i18n/I18nContext";

export default function FaceRating({ stars }: { stars: 1 | 2 | 3 }) {
  const { t } = useI18n();
  const label = t("face.starsAdvisory", { count: stars });
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
