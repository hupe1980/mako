import {
  SearchField as AriaSearchField,
  Button,
  Input,
  type SearchFieldProps,
} from "react-aria-components";
import { Icon } from "./Icon";

/** The search field: large on a view's top, compact in chrome. */
export function SearchField({
  placeholder,
  size = "large",
  ...props
}: SearchFieldProps & { placeholder: string; size?: "large" | "compact" }) {
  const large = size === "large";
  return (
    <AriaSearchField {...props} className="group relative flex items-center">
      <Icon
        name="search"
        size={large ? 18 : 15}
        className={`pointer-events-none absolute text-text-2 ${large ? "left-3.5" : "left-2.5"}`}
      />
      <Input
        placeholder={placeholder}
        className={`focus-ring squircle w-full rounded-control border border-separator bg-surface text-text placeholder:text-text-2 [&::-webkit-search-cancel-button]:hidden ${
          large ? "h-11 pl-11 pr-10 text-callout" : "h-7 pl-8 pr-8 text-body"
        }`}
      />
      <Button
        aria-label="Leeren"
        className="focus-ring absolute right-2 hidden size-6 items-center justify-center rounded-full text-text-2 group-data-[empty=false]:flex data-[hovered]:bg-fill"
      >
        ×
      </Button>
    </AriaSearchField>
  );
}
