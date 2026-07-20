import { cn } from "../lib/utils";

import type { OriginAttribution } from "./types";

/** Props for the delegated-origin badge. */
export interface OriginBadgeProps {
  /** Origin attribution from a thread or interaction item. */
  readonly origin?: OriginAttribution;
  /** Additional class names. */
  readonly className?: string;
}

/** Renders `[from <delegate>@depth<n>]` for non-root delegated work. */
export function OriginBadge({ className, origin }: OriginBadgeProps): React.JSX.Element | null {
  if (origin?.delegate === undefined) {
    return null;
  }

  return (
    <span
      className={cn(
        "inline-flex items-center rounded-full border border-border bg-muted px-2 py-0.5 text-[11px] font-medium text-muted-foreground",
        className
      )}
    >
      [from {origin.delegate}@depth{origin.depth}]
    </span>
  );
}
