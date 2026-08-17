import type { ButtonHTMLAttributes } from "react";
import { cn } from "@/lib/cn";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "default" | "secondary" | "destructive" | "ghost";
  size?: "sm" | "default" | "lg";
}

const baseStyles =
  "inline-flex items-center justify-center gap-2 rounded-md font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:opacity-50 disabled:pointer-events-none";

const variantStyles = {
  default: "bg-primary text-primary-foreground hover:bg-primary/90",
  secondary: "border border-border bg-secondary text-secondary-foreground hover:bg-accent",
  destructive: "bg-destructive text-destructive-foreground hover:bg-destructive/90",
  ghost: "text-foreground hover:bg-accent",
};

const sizeStyles = {
  sm: "h-8 px-3 text-sm",
  default: "h-10 px-4 text-sm",
  lg: "h-12 px-6 text-base",
};

export function Button({
  className,
  variant = "default",
  size = "default",
  disabled,
  ...props
}: ButtonProps) {
  return (
    // A `<button>` with no `type` is a submit button, so one placed in a form without thinking
    // about it submits the form. The literal here is the default; `{...props}` comes after it, so
    // a caller that means to submit still says `type="submit"` and wins.
    <button
      type="button"
      className={cn(baseStyles, variantStyles[variant], sizeStyles[size], className)}
      disabled={disabled}
      {...props}
    />
  );
}
