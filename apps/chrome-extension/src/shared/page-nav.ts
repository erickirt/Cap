export const PAGE_NAV_LINKS = [
	{ id: "welcome", label: "Welcome", href: "welcome.html" },
	{ id: "how-it-works", label: "How it works", href: "how-it-works.html" },
	{ id: "camera", label: "Camera access", href: "camera-permission.html" },
	{ id: "options", label: "Options", href: "options.html" },
] as const;

export type PageNavId = (typeof PAGE_NAV_LINKS)[number]["id"];

export const mountPageNav = (active: PageNavId) => {
	if (document.querySelector(".page-nav")) return;

	const nav = document.createElement("nav");
	nav.className = "page-nav";
	nav.setAttribute("aria-label", "Cap extension pages");

	const inner = document.createElement("div");
	inner.className = "page-nav-inner";

	const brand = document.createElement("a");
	brand.className = "page-nav-brand";
	brand.href = "welcome.html";
	brand.setAttribute("aria-label", "Cap");
	const logo = document.createElement("img");
	logo.src = "icons/icon-32.png";
	logo.alt = "";
	logo.width = 26;
	logo.height = 26;
	brand.append(logo);

	const links = document.createElement("div");
	links.className = "page-nav-links";
	for (const link of PAGE_NAV_LINKS) {
		const anchor = document.createElement("a");
		anchor.className =
			link.id === active ? "page-nav-link is-active" : "page-nav-link";
		anchor.href = link.href;
		anchor.textContent = link.label;
		if (link.id === active) {
			anchor.setAttribute("aria-current", "page");
		}
		links.append(anchor);
	}

	inner.append(brand, links);
	nav.append(inner);
	document.body.prepend(nav);
};
