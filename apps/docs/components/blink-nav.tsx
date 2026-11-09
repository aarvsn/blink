import Link from 'next/link';

export function BlinkLogo({ className = '' }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden
      className={`h-5 w-5 ${className}`}
    >
      <defs>
        <linearGradient id="blinkGradNav" x1="0" y1="0" x2="24" y2="24" gradientUnits="userSpaceOnUse">
          <stop stopColor="#6866ff" />
          <stop offset="1" stopColor="#3f3e9e" />
        </linearGradient>
      </defs>
      <circle cx="12" cy="12" r="11" fill="url(#blinkGradNav)" />
      <path d="M7 12.5 L10.5 12.5 L13 6 L11 11 L17 11 L11.5 18 L13 12.5 Z" fill="white" />
    </svg>
  );
}

export function BlinkNav() {
  return (
    <div className="flex items-center justify-between gap-4 px-4 py-3">
      <Link
        href="/docs"
        className="inline-flex items-center gap-2 text-sm font-semibold text-fd-foreground"
      >
        <BlinkLogo />
        <span>
          Blink <span className="text-fd-muted-foreground">Docs</span>
        </span>
      </Link>
      <div className="flex items-center gap-3 text-sm text-fd-muted-foreground">
        <Link href="/docs/installation" className="hover:text-fd-foreground">
          Installation
        </Link>
        <Link
          href="https://github.com/aarvsn/blink"
          target="_blank"
          rel="noreferrer"
          className="hover:text-fd-foreground"
        >
          GitHub
        </Link>
      </div>
    </div>
  );
}
