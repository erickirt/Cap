import { promises as fs } from "node:fs";
import path from "node:path";
import { getBlogPosts, getDocs } from "@/utils/blog";
import { seoPages } from "../lib/seo-pages";

async function getPagePaths(
	dir: string,
): Promise<{ path: string; lastModified: string }[]> {
	const entries = await fs.readdir(dir, { withFileTypes: true });
	const paths: { path: string; lastModified: string }[] = [];

	for (const entry of entries) {
		const fullPath = path.join(dir, entry.name);
		if (
			entry.isDirectory() &&
			entry.name !== "dashboard" &&
			entry.name !== "blog" &&
			!entry.name.startsWith("[")
		) {
			const subPaths = await getPagePaths(fullPath);
			paths.push(...subPaths);
		} else if (
			entry.isFile() &&
			(entry.name === "page.tsx" || entry.name === "page.mdx")
		) {
			const relativePath = path.relative(process.cwd(), dir);
			// Filter out route groups (directories wrapped in parentheses) and construct clean path
			const pathSegments = relativePath
				.split(path.sep)
				.slice(1)
				.filter(
					(segment) => !(segment.startsWith("(") && segment.endsWith(")")),
				);
			const routePath =
				pathSegments.length > 0 ? `/${pathSegments.join("/")}` : "/";

			if (!routePath.includes("/dashboard") && !routePath.includes("[")) {
				const stats = await fs.stat(fullPath);
				paths.push({
					path: routePath === "/app" ? "/" : routePath,
					lastModified: stats.mtime.toISOString(),
				});
			}
		}
	}

	return paths;
}

export default async function sitemap() {
	const appDirectory = path.join(process.cwd(), "app");
	const pagePaths = await getPagePaths(appDirectory);

	// Add blog post routes. Prefer updatedAt (set when a post is refreshed) over
	// publishedAt so the sitemap reports a realistic last-modified date.
	const blogPosts = getBlogPosts();
	const blogRoutes = blogPosts.map((post) => {
		const meta = post.metadata as { publishedAt?: string; updatedAt?: string };
		const stamp =
			meta.updatedAt ?? meta.publishedAt ?? new Date().toISOString();
		const publishDate = new Date(stamp);
		publishDate.setHours(9, 0, 0, 0); // normalize to 9:00 AM
		return {
			path: `/blog/${post.slug}`,
			lastModified: publishDate.toISOString(),
		};
	});

	// Add docs routes
	const docs = getDocs();
	const docsRoutes = docs.map((doc) => ({
		path: `/docs/${doc.slug}`,
		lastModified: new Date().toISOString(), // You might want to add a publishedAt to doc metadata
	}));

	// SEO content pages are physical App Router pages under app/(site)/(seo) and
	// app/(site)/solutions, so getPagePaths() already discovers each one with a
	// real file mtime — those physical pages are the source of truth. The seoPages
	// registry is kept only as a fallback: emit a slug from it ONLY when the
	// filesystem walk didn't already produce that path. This is what stops the
	// same SEO URL being emitted twice (once from disk, once from the registry).
	const discoveredPaths = new Set(pagePaths.map((route) => route.path));
	const seoRoutes = Object.keys(seoPages)
		.map((slug) => `/${slug}`)
		.filter((routePath) => !discoveredPaths.has(routePath))
		.map((routePath) => ({ path: routePath, lastModified: undefined }));

	// Combine, dedupe by path (first occurrence wins, so filesystem mtimes are
	// preserved), and ensure '/' is first.
	const combined = [...pagePaths, ...blogRoutes, ...docsRoutes, ...seoRoutes];
	const uniqueByPath = new Map<
		string,
		{ path: string; lastModified?: string }
	>();
	for (const route of combined) {
		if (!uniqueByPath.has(route.path)) uniqueByPath.set(route.path, route);
	}
	const dedupedRoutes = [...uniqueByPath.values()];
	const homeRoute = dedupedRoutes.find((route) => route.path === "/");
	const otherRoutes = dedupedRoutes.filter((route) => route.path !== "/");

	return [...(homeRoute ? [homeRoute] : []), ...otherRoutes].map((route) => ({
		url: `https://cap.so${route.path}`,
		...(route.lastModified ? { lastModified: route.lastModified } : {}),
	}));
}
