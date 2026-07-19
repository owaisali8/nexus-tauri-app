import type { ReactNode } from "react";
import type { AppView } from "./types";

type IconProps = {
  className?: string;
};

function IconBase({ className, children }: IconProps & { children: ReactNode }) {
  return (
    <svg
      className={className}
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

export function ProvidersIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M12 3v3" />
      <path d="M6.3 5.7 8.5 7.9" />
      <path d="M3 12h3" />
      <path d="M5.7 17.7l2.2-2.2" />
      <path d="M12 18v3" />
      <path d="M17.7 17.7l-2.2-2.2" />
      <path d="M18 12h3" />
      <path d="M15.5 7.9l2.2-2.2" />
      <circle cx="12" cy="12" r="4" />
    </IconBase>
  );
}

export function AgentsIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <rect x="4" y="8" width="16" height="12" rx="2" />
      <circle cx="9" cy="13" r="1.25" fill="currentColor" stroke="none" />
      <circle cx="15" cy="13" r="1.25" fill="currentColor" stroke="none" />
      <path d="M9 17h6" />
      <path d="M12 8V5" />
      <circle cx="12" cy="4" r="1.5" />
    </IconBase>
  );
}

export function ChatIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M21 12a8 8 0 0 1-8 8H7l-4 3V12a8 8 0 0 1 8-8h4a8 8 0 0 1 8 8Z" />
      <path d="M8 12h8" />
      <path d="M8 9.5h5" />
    </IconBase>
  );
}

export function FilesIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M14 3H8a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V8Z" />
      <path d="M14 3v5h5" />
      <path d="M9 13h6" />
      <path d="M9 17h4" />
    </IconBase>
  );
}

export function MemoryIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M12 4c-3 0-5 2.2-5 5.2 0 2.2 1.2 3.4 2.4 4.4.9.8 1.8 1.5 2.1 2.8l.5 2.1.5-2.1c.3-1.3 1.2-2 2.1-2.8 1.2-1 2.4-2.2 2.4-4.4C17 6.2 15 4 12 4Z" />
      <path d="M9.5 20.5c.8.5 1.7.5 2.5 0" />
    </IconBase>
  );
}

const navIconMap: Record<AppView, (props: IconProps) => ReactNode> = {
  providers: ProvidersIcon,
  agents: AgentsIcon,
  chat: ChatIcon,
  files: FilesIcon,
  memory: MemoryIcon,
};

export function NavIcon({ view, className }: { view: AppView; className?: string }) {
  const Icon = navIconMap[view];
  return <Icon className={className} />;
}
