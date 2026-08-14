import type { HTMLAttributes } from "react";
import { cn } from "@/lib/cn";

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  variant?: "default" | "secondary" | "outline" | "destructive";
}

const baseStyles =
  "inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-ring";

const variantStyles = {
  default: "border border-transparent bg-primary text-primary-foreground",
  secondary: "border border-transparent bg-secondary text-foreground",
  outline: "border border-input bg-transparent text-foreground",
  destructive: "border border-transparent bg-destructive text-destructive-foreground",
};

export function Badge({ className, variant = "default", ...props }: BadgeProps) {
  return <span className={cn(baseStyles, variantStyles[variant], className)} {...props} />;
}
