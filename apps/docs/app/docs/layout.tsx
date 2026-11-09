import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import type { ReactNode } from 'react';
import { source } from '@/lib/source';
import { BlinkLogo } from '@/components/blink-nav';

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <DocsLayout
      tree={source.getPageTree()}
      nav={{
        enabled: true,
        title: (
          <span className="inline-flex items-center gap-2 text-sm font-semibold text-fd-foreground">
            <BlinkLogo />
            <span>
              Blink <span className="text-fd-muted-foreground">Docs</span>
            </span>
          </span>
        ),
        url: '/docs',
      }}
      links={[
        {
          text: 'Installation',
          url: '/docs/installation',
        },
        {
          text: 'GitHub',
          url: 'https://github.com/aarvsn/blink',
          external: true,
        },
      ]}
      sidebar={{
        tabs: false,
      }}
    >
      {children}
    </DocsLayout>
  );
}
