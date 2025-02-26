import { notFound } from "next/navigation";
import Image from "next/image";
import { format, parseISO } from "date-fns";
import { MDXRemote } from "next-mdx-remote/rsc";
import { getBlogPosts } from "@/utils/blog";
import type { Metadata } from "next";
import { Share } from "../_components/Share";
import { ReadyToGetStarted } from "@/components/ReadyToGetStarted";
import { calculateReadingTime } from "@/utils/readTime";
import { clientEnv } from "@cap/env";

interface PostProps {
  params: {
    slug: string;
  };
}

export async function generateMetadata({
  params,
}: PostProps): Promise<Metadata | undefined> {
  let post = getBlogPosts().find((post) => post.slug === params.slug);
  if (!post) {
    return;
  }

  let { title, publishedAt: publishedTime, description, image } = post.metadata;
  let ogImage = `${clientEnv.NEXT_PUBLIC_WEB_URL}${image}`;

  return {
    title,
    description,
    openGraph: {
      title,
      description,
      type: "article",
      publishedTime,
      url: `${clientEnv.NEXT_PUBLIC_WEB_URL}/blog/${post.slug}`,
      images: [
        {
          url: ogImage,
        },
      ],
    },
    twitter: {
      card: "summary_large_image",
      title,
      description,
      images: [ogImage],
    },
  };
}

export default async function PostPage({ params }: PostProps) {
  const post = getBlogPosts().find((post) => post.slug === params.slug);

  if (!post) {
    notFound();
  }

  const readingTime = calculateReadingTime(post.content);

  return (
    <>
      <article className="py-8 prose mx-auto ">
        {post.metadata.image && (
          <div className="relative mb-12 h-[345px] w-full">
            <Image
              className="m-0 w-full rounded-lg object-contain sm:object-cover"
              src={post.metadata.image}
              alt={post.metadata.title}
              fill
              priority
            />
          </div>
        )}

        <div className="wrapper">
          <header>
            <h1 className="mb-2">{post.metadata.title}</h1>
            <p className="space-x-1 text-xs text-gray-500">
              <span>
                {format(parseISO(post.metadata.publishedAt), "MMMM dd, yyyy")}
              </span>
              <span>—</span>
              <span>{readingTime} min read</span>
            </p>
          </header>
          <hr className="my-6" />
          <MDXRemote source={post.content} />
          <Share post={post} />
        </div>
      </article>
      <div className="wrapper mb-4">
        <ReadyToGetStarted />
      </div>
    </>
  );
}
