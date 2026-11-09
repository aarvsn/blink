import {
  DocsBody,
  DocsDescription,
  DocsPage,
  DocsTitle,
} from 'fumadocs-ui/layouts/docs/page';
import { notFound } from 'next/navigation';
import { getMDXComponents } from '@/mdx-components';
import { source } from '@/lib/source';
import type { Metadata } from 'next';
import type { TOCItemType } from 'fumadocs-core/toc';
import type { MDXContent } from 'mdx/types';

/**
 * Runtime shape of a fumadocs-mdx page. TypeScript 5 cannot follow the
 * `defineDocs` macro's generics through the loader, so the contract that the
 * compiler produces at runtime is spelled out here instead.
 */
interface DocPageData {
  title: string;
  description?: string;
  toc: TOCItemType[];
  full?: boolean;
  body: MDXContent;
}

export default async function Page(props: {
  params: Promise<{ slug?: string[] }>;
}) {
  const params = await props.params;
  const page = source.getPage(params.slug);
  if (!page) notFound();

  const data = page.data as unknown as DocPageData;
  const MDXContent = data.body;

  return (
    <DocsPage toc={data.toc} full={data.full}>
      <DocsTitle>{data.title}</DocsTitle>
      <DocsDescription className="mb-0">{data.description}</DocsDescription>
      <DocsBody>
        <MDXContent components={getMDXComponents()} />
      </DocsBody>
    </DocsPage>
  );
}

export function generateStaticParams() {
  return source.generateParams();
}

export async function generateMetadata(props: {
  params: Promise<{ slug?: string[] }>;
}): Promise<Metadata> {
  const params = await props.params;
  const page = source.getPage(params.slug);
  if (!page) notFound();

  const data = page.data as unknown as DocPageData;
  return {
    title: data.title,
    description: data.description,
  };
}
