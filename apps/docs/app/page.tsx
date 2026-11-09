import Link from 'next/link';
import { BlinkLogo } from '@/components/blink-nav';

const sections = [
  {
    href: '/docs/installation',
    title: 'Installation',
    body: 'Install the panel and at least one blinkd node with the automated installer or by hand.',
  },
  {
    href: '/docs/panel-configuration',
    title: 'Panel configuration',
    body: 'Environment variables, the admin account, reverse proxies and HTTPS for wss consoles.',
  },
  {
    href: '/docs/units',
    title: 'Units',
    body: 'The unit format v3: Docker images, startup commands, variables, install scripts and egg imports.',
  },
  {
    href: '/docs/blinkd-configuration',
    title: 'blinkd configuration',
    body: 'blinkd.yml reference, the configure wizard and running the daemon under systemd.',
  },
];

export default function HomePage() {
  return (
    <main className="flex-1 bg-fd-background text-fd-foreground">
      <div className="relative overflow-hidden">
        <div
          aria-hidden
          className="pointer-events-none absolute inset-x-0 -top-40 h-80 opacity-30 blur-3xl"
          style={{
            background: 'radial-gradient(50% 60% at 50% 50%, #6866ff 0%, transparent 100%)',
          }}
        />
        <div className="mx-auto flex max-w-5xl flex-col items-center px-6 pb-16 pt-24 text-center">
          <BlinkLogo className="h-12 w-12" />
          <h1 className="mt-6 text-4xl font-bold tracking-tight sm:text-5xl">
            Blink Documentation
          </h1>
          <p className="mt-4 max-w-2xl text-base text-fd-muted-foreground">
            Blink is a game server management platform: a self-hosted panel for
            organising nodes and servers, and <code className="font-mono text-sm">blinkd</code>, a
            Rust daemon that runs each game server in its own container.
          </p>
          <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
            <Link
              href="/docs/introduction"
              className="rounded-md bg-fd-primary px-4 py-2 text-sm font-semibold text-white transition hover:opacity-90"
            >
              Get started
            </Link>
            <Link
              href="/docs/installation"
              className="rounded-md border border-fd-border px-4 py-2 text-sm font-medium text-fd-secondary-foreground transition hover:bg-fd-accent"
            >
              Install Blink
            </Link>
          </div>
        </div>
      </div>

      <div className="mx-auto grid max-w-5xl gap-4 px-6 pb-24 sm:grid-cols-2">
        {sections.map((section) => (
          <Link
            key={section.href}
            href={section.href}
            className="group rounded-lg border border-fd-border bg-fd-card p-6 transition hover:border-fd-primary/60"
          >
            <h2 className="text-sm font-semibold text-fd-foreground group-hover:text-fd-primary">
              {section.title}
            </h2>
            <p className="mt-2 text-sm leading-relaxed text-fd-muted-foreground">
              {section.body}
            </p>
          </Link>
        ))}
      </div>
    </main>
  );
}
