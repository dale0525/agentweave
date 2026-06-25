import { ReactNode } from "react";

type AppIconButtonProps = {
  children: ReactNode;
  disabled?: boolean;
  label: string;
  onClick?: () => void;
  type?: "button" | "submit";
};

export function AppIconButton({
  children,
  disabled = false,
  label,
  onClick,
  type = "button"
}: AppIconButtonProps): JSX.Element {
  return (
    <button
      aria-label={label}
      className="icon-button"
      disabled={disabled}
      onClick={onClick}
      title={label}
      type={type}
    >
      {children}
    </button>
  );
}
