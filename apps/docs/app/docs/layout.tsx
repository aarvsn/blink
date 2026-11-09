import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import type { ReactNode } from 'react';
import { source } from '@/lib/source';
import { BlinkNav } from '@/components/blink-nav';

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <DocsLayout
      tree={source.getPageTree()}
      nav={{
        enabled: true,
        component: <BlinkNav />,
      }}
      sidebar={{
        tabs: false,
      }}
    >
      {children}
    </DocsLayout>
  );
}
