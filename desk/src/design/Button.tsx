import { Button as AriaButton, type ButtonProps } from "react-aria-components";

type Variant = "primary" | "secondary" | "ghost" | "destructive";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent text-on-accent data-[hovered]:brightness-110 data-[pressed]:brightness-95",
  secondary: "bg-fill text-text data-[hovered]:bg-fill-strong data-[pressed]:bg-fill-strong",
  ghost:
    "text-text-2 data-[hovered]:bg-fill data-[hovered]:text-text data-[pressed]:bg-fill-strong",
  destructive:
    "bg-critical text-on-accent data-[hovered]:brightness-110 data-[pressed]:brightness-95",
};

/**
 * A push button. Primary is the one action a screen is for; ghost lives in
 * chrome; destructive refuses, rejects or discards — styled so, never confirmed
 * by a dialog where a hold is the way back.
 */
export function Button({
  variant = "secondary",
  className = "",
  ...props
}: ButtonProps & { variant?: Variant; className?: string }) {
  return (
    <AriaButton
      {...props}
      className={`focus-ring squircle inline-flex h-7 items-center gap-1.5 rounded-control px-3 text-body font-medium transition-[background-color,filter] duration-100 data-[disabled]:opacity-40 ${VARIANTS[variant]} ${className}`}
    />
  );
}
