import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "../lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center rounded-lg text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary disabled:pointer-events-none disabled:opacity-50",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-primary/90",
        outline: "border border-border bg-background hover:bg-foreground/5"
      },
      size: {
        default: "h-10 px-4 py-2",
        sm: "h-8 px-3"
      }
    },
    defaultVariants: {
      size: "default",
      variant: "default"
    }
  }
);

/** Props for the shared mag button primitive. */
export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>, VariantProps<typeof buttonVariants> {}

/** Minimal shadcn-style button primitive used to validate the UI package pipeline. */
export function Button({ className, size, variant, ...props }: ButtonProps): React.JSX.Element {
  return <button className={cn(buttonVariants({ className, size, variant }))} {...props} />;
}
